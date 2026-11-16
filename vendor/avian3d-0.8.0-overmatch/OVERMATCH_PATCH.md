# Overmatch avian3d patch

This directory is the published `avian3d` 0.8.0 crate with the source changes inventoried below.

Produced from `https://static.crates.io/crates/avian3d/avian3d-0.8.0.crate`, whose sha256
`ca7479ccec5f1440675946bada8c805f2b6d514af78e1d2b0937ca53b1e2c543` matches the crates.io index's
`cksum`, by `tar -xzf avian3d-0.8.0.crate -C vendor/avian3d-0.8.0-overmatch --strip-components=1
--exclude='*/Cargo.lock' --exclude='*/.cargo_vcs_info.json'`. The published lockfile and VCS record
are omitted; neither is read for a path dependency.

Subject: the game's server physics (rigid bodies, contacts, rest). Game state only.

## How the fork is shaped

Each patch takes one form, named in its section:

- **hook**: a narrow extension point inside an upstream file; the law it serves lives in the game
  (`src/avian_ext/`, one file per seam, installed by `avian_ext::plugin` after
  `PhysicsPlugins`).
- **module**: a self-contained fork file with a few call sites in upstream files.
- **game-side**: no fork line; the game attaches through public schedule slots, components and
  resources.
- **kept**: an edit inside an upstream file, with the reason no narrower form exists.
- **dropped**: upstream 0.8.0 does it; the old witness runs against unpatched code.

Patched sites in upstream files are tagged `OVERMATCH PATCH`. Fork lines are counted by `diff -ru`
of `src/` against the published crate: 880 in all (MEASURED), 802 in upstream files and 78 in the
fork-only `src/collision/narrow_phase/stationary_miss.rs`.

Guards run as the game's own tests, each named in its patch's section.

## Child collider poses (`src/collision/collider/collider_transform/plugin.rs`): hook

The upstream system and registration remain intact. The fork makes
`update_child_collider_position` public and re-exports it (7 fork lines, MEASURED against pristine
0.8.0 with `diff -ruN src/`). The game removes exactly one instance in
`PhysicsPlacementPlugin::cleanup`, after every plugin has built and finished, then installs its
change-aware placement in the same slot. Missing registration fails during setup.

The game also replaces the published Lightyear integration's frame-boundary writer from
`FramePlacementPlugin::cleanup`; no lightyear_avian3d fork remains. `ColliderTransformPlugin`
stays enabled for transform propagation and ancestry marking.

Guard: `src/tests/avian_child_pose_churn.rs`, including the shipped composition's schedule.
Record: `upstream/avian-child-collider-pose-churn.md`.

## Attachment refusal: game-side

`ColliderOf::on_insert` returns without listing the collider in its body's `RigidBodyColliders`
when the collider or the body has no `GlobalTransform`. Every child placement boundary reaches a
moved body's colliders through that list, so such a child stays at its birth pose. Avian's
`PhysicsTransformPlugin` requires `Transform` through `Position`; the game's networked composition
disables that plugin, so there a body spawned without `Transform` takes this path silently.

The game refuses it in an observer on `Insert<ColliderOf>` (`avian_ext::attachment`), which runs
after the hook on the same insertion and panics naming both entities and the fix. It reads both
entities raw, as the hook does, so a disabled collider or body passes it as it passes the hook. The
0.7 fork panicked inside the hook. Cost of the observer, MEASURED on this fork in an unoptimized
test build, 10 000 attachments spawned onto 2 000 static bodies, three runs each: 305 to 310 ms
without it, 315 to 316 ms with it, about 2%; the hook form is not worth its fork lines.

Guards: `src/tests/avian_child_pose_churn.rs`,
`a_collider_attached_to_a_body_without_a_global_transform_is_refused` (stock 0.8.0 does not panic,
MEASURED) and `a_posed_attachment_stands_with_either_side_disabled` (red with the observer reading
through default-filtered queries, which skip a disabled entity, MEASURED).

## Prepared collider mass (`src/collision/collider/parry/mod.rs`): kept

`Collider::with_mass_properties_cache(density)` computes the current scaled shape's mass
properties before insertion; `ComputeMassProperties::mass_properties` returns that value for the
same density, and shape mutation, shape replacement and a changed scale drop it. The cache is cloned
with the collider and skipped by serde. Sensors still install zero mass.

Kept, because the value has to ride the collider into Avian's insertion hook: the `C` insert hook
in `collision/collider/backend.rs` computes `C::mass_properties` and overwrites any
`ColliderMassProperties` already on the entity, so a precomputed component is no attachment point,
and short of a custom `AnyCollider` there is none. 21 fork lines (MEASURED).

The game prepares the cache off the system thread for each construction chunk collider that is no
sensor. Guard: `src/tests/avian_prepared_mass.rs`, which replaces the shape of a freshly prepared
collider, so each invalidation stands alone (it fails with the scale invalidation removed and with
the shape replacement's removed, MEASURED). Local upstream draft:
`upstream/avian-prepared-collider-mass.md`.

## Complete collider batches and explicit mass ownership (`src/collision/collider/collider_hierarchy/plugin.rs`, `src/dynamics/rigid_body/mass_properties/`, `src/lib.rs`): kept

`collider_hierarchy/plugin.rs` keeps an existing `ColliderOf` on a hierarchy change when it
already names the nearest body, so a complete spawn bundle supplies the attachment beside its shape
and enters the body's collider tree once. Upstream's `On<Insert<(ChildOf, RigidBody,
AncestorMarker<ColliderMarker>)>>` observer is split in two instances of one generic function:
`ChildOf` and `AncestorMarker` skip an attachment that already names the body; `RigidBody` keeps
upstream's unconditional re-insert, which repairs colliders that arrived before their body.
`AncestorMarker` is re-exported from `lib.rs` so a prepared group marks its collider ancestry at
spawn instead of by a deferred write and subtree walk per member.

The game's group constructor inserts the body, with an empty `RigidBodyColliders`, before any of
its colliders; Avian's relationship hook then appends each collider directly. Bevy 0.20's
`spawn_batch` and `insert_batch` keep per-entity hooks and observers: no hook, shape initialization
or tree leaf is bypassed.

`mass_properties/` treats a body as fully explicit only when `Mass`, `AngularInertia`,
`CenterOfMass` and the three matching `NoAuto*` markers are all present. Its membership changes do
not recompute those outputs, its update does not aggregate descendants whose contributions would all
be replaced, and collider changes queue one request per other owner; a self-attached collider still
queues its owner's own explicit edits. Partially explicit and automatic bodies aggregate as
upstream.

Kept: it changes what Avian's own observers and mass systems do, and none of them has an extension
point. 66 fork lines (MEASURED).

Guards: `src/tests/avian_spawn_batch.rs`, 4 tests. On stock 0.8.0 two fail (MEASURED): a complete
bundle's attachment is installed 4 times, not once, and a change to two children's mass queues 2
aggregates for one body. The other two guard upstream behaviour the patch keeps (a late body's
repair; a self-attached explicit body's edits). The construction's
`departure_and_split_preserve_each_chunks_collider_contract` guards the game's use.

## Pairless-layer broad-phase prune (`src/collision/broad_phase/bvh_broad_phase.rs`, `src/collision/diagnostics.rs`): kept

`collect_collision_pairs` skips a moved proxy's tree traversals when its collision-layer
memberships or filters are empty: either empty term makes `CollisionLayers::interacts_with` false
for every partner. The proxy stays in its one collider tree, so spatial queries still find it.
Eligibility reads the proxy's current layers each step, sensor and standalone proxies included.
The system reads `ColliderTrees` as `Res` (upstream asks `ResMut` and writes nothing), and a live
moved proxy without bounds panics where upstream skipped it unseen. Upstream's own `unwrap`s stay.

`CollisionDiagnostics::broad_phase_candidates` counts non-self BVH leaf visits before duplicate,
layer and body filtering, summed from per-chunk counts; a candidate can be visited from both
endpoints. The game's tick trace reads it for the ram and scheduled-collapse instruments.

Kept: the prune sits inside the broad phase's private per-proxy loop, so the alternative is
replacing the whole broad-phase plugin. 33 fork lines (MEASURED: 28 in the broad phase, 5 in the
diagnostics).

Guard: `src/tests/avian_broad_phase_layers.rs` (with the prune removed it fails: 2 candidates
visited, not 0, MEASURED). Counts and reproduction: `upstream/avian-pairless-broad-phase.md`.

## Sleeping bodies' velocity increments (`src/dynamics/integrator/mod.rs`, `src/dynamics/rigid_body/forces/plugin.rs`, `src/dynamics/solver/solver_body/mod.rs`): kept

Subject: the game's rigid-body rest, server physics, game state only.

World-frame increment construction (constant forces and accelerations, gravity, and substep
scaling) uses `With<SolverBodyIndex>`, the same membership as consumption and clearing. A body
outside the solver keeps exactly its pending increment until wake, unscaled. `ConstantTorque`
already requires the index; local acceleration applies only through solver indices and retains
upstream's separate accumulation and clear.

The filter avoids building increments for sleepers at all. No hold/release adapter scans the
sleeping population or copies its pending increments. MEASURED by the supplied cutover profile:
the removed adapter cost about 32 µs per tick; no new timing run or speedup claim here.

The filter is exact only while the membership holds from the build to the clear: a body entering
in between would take its unscaled pending acceleration as a velocity every substep, and one
leaving would keep a scaled increment no clear reaches. `SolverBodies` counts its membership
changes (every push, removal and clear); `pre_process_velocity_increments` stores the count in
`IncrementsBuiltFor`, and `clear_velocity_increments` panics if it moved. Two reads and a compare
per step.

Kept: the filters sit inside upstream systems, and the count is bumped inside `SolverBodies`, whose
membership only its own methods change. 75 fork lines (MEASURED: 44 in the integrator, 18 in the
force builders, 13 in `SolverBodies`).

Guards: `src/tests/avian_island_sleep.rs` (`a_woken_body_takes_only_its_first_steps_gravity`,
`a_force_on_a_sleeper_acts_in_its_first_step_awake`,
`sleeping_increments_wait_untouched_and_act_on_wake`,
`a_body_woken_between_the_increments_build_and_clear_is_refused`). The third checks pending data
and change ticks across steps out of the solver (asleep, disabled and static), then the force's
contribution on wake; the fourth wakes a body inside the window. Mutants, each red (MEASURED): the
four builders filtered `Without<Sleeping>` (the third: the disabled and static bodies' increments
and change ticks move), the clear's check removed (the fourth does not panic). Local upstream
draft: `upstream/avian-sleeping-velocity-increments.md`.

## Unreadable pair state (`src/collision/narrow_phase/system_param.rs`): kept

`update_contacts` reads a pair's two colliders and their bodies. It skips a pair whose colliders it
cannot fetch only when one is of another collider type (that type's narrow phase updates the pair)
or unposed (a client's replication lifted its pose for a frame); disabled and removed colliders
lose their pairs first (`remove_collider_on`). It reads a body outside its body query as no body
only when the body is disabled (`RigidBodyDisabled` or `Disabled`, whose pairs generate no
constraints) or unposed. Anything else has lost a component every collider or body carries, and
panics naming the pair or the body; upstream skips the pair or reads the body as absent, unseen.

Kept: both reads are inside the private per-pair closure. 57 fork lines (MEASURED; two filter
queries on `NarrowPhase` and the two checks).

Guards: `src/tests/avian_narrow_phase.rs` (two refusals, red on stock 0.8.0, MEASURED; two
legitimate absences). Upstream's own defect on the same path, a body leaving and re-entering the
body query, still panics the solver on 0.8.0 (MEASURED by the tripwire
`src/tests/avian_body_query_gap.rs`); record: `upstream/avian-body-query-gap-stale-constraints.md`.

## Disabled colliders in continuous collision (`src/dynamics/ccd/mod.rs`): kept until upstream releases #1101

`solve_continuous` fetches each fast body's colliders through a query without
`Without<ColliderDisabled>`, so a disabled collider still attached to a fast body (its
`RigidBodyColliders` keeps it) is swept and can stop the body at an impact it does not take part in.
Upstream fixed it after 0.8.0 (avian #1101) by adding the filter; the fork carries that line. 6 fork
lines (MEASURED, the query reformatted). No guard of the game's; drop the patch with the release
that carries #1101.

## Moved-collider bounds prune (`src/collider_tree/update.rs`, `src/collider_tree/diagnostics.rs`): kept

`update_moved_collider_aabbs` (`ColliderTreeSystems::UpdateAabbs`, every physics step) fetches
every enabled collider's pose, shape, bounds and proxy key, then skips those whose `Position`,
`Rotation` and shape ticks are not newer than `LastPhysicsTick`. On the product map that is
thousands of standing static colliders fetched every step for nothing.

The patch adds a third query to the system's `ParamSet`, the same data filtered
`Or<(Changed<Position>, Changed<Rotation>, Changed<C>)>`, so a collider whose ticks did not move
since the system's previous run costs the filter's three tick reads. Upstream's per-collider body,
its `LastPhysicsTick` check included, is unchanged and runs for every collider the filter passes.
That is exact while `LastPhysicsTick` is newer than the system's previous run: every tick newer
than `LastPhysicsTick` is then newer than that run, so the filter passes every collider the check
accepts. `update_last_physics_tick` runs after this system in every step, so inside the physics
schedule it always holds; the game's other runs of the system (ballistics and the construction's
refit, between steps) take the unfiltered query whenever it does not. The filtered query iterates
with Bevy's `par_iter_mut` directly: Avian's `par_for_each_mut` counts its query first, which for
a change filter is a second pass over every collider.

`ColliderTreeDiagnostics::moved_aabb_candidates` counts the colliders the pass examined this step,
per thread and summed once; it is the one observable of the prune.

Kept: a query filter inside one upstream system, which has no extension point; replacing the
system means replacing the plugin. 72 fork lines (MEASURED: 63 in the update, 9 in the
diagnostics).

Guard: `src/tests/avian_moved_aabb_prune.rs` (standing static colliders are not examined, red with
the prune bypassed, MEASURED: 4 examined; a static collider moved, rescaled, added or removed
reaches the collider trees). Record: `upstream/avian-moved-aabb-prune.md`.

## Contact recycling: off in the game

Not a patch: a configuration several patches and game seams rely on. A pair Avian recycles
(`NarrowPhaseConfig::recycle_distance` above zero, its bodies' relative motion small) returns from
the narrow phase's per-pair update before the geometry query, warm-start matching, the
stationary-miss cache and the pair's collision hooks, keeping last step's manifolds. The game runs
with recycling off, set where the game composes physics (`src/net/physics.rs`, guard
`shared_physics_keeps_contact_recycling_off`); the guard fixtures here do the same. Turning it on
stops warm-start matching and the stationary cache for recycled pairs, leaves a reduced pair's lost
manifolds unregenerated while it rests, and skips the belt's `WorldContacts::modify_contacts`
(`src/track/stop_contacts.rs`), which must run every step for every candidate belt-face pair: those
pairs at least would have to stay exempt.

## Minimum speculative margin: dropped

The 0.7 fork floored the speculative margin shared by the geometry query and point pruning at the
configured `contact_tolerance`. Avian 0.8.0 clamps it itself ("Ensure that the speculative distance
is at least as large as the contact tolerance"). Witness:
`stationary_contacts_keep_the_configured_minimum_speculative_margin` in
`src/tests/avian_island_sleep.rs`, green on this fork, which carries no line for it (MEASURED).

## Serial small loops: dropped

Subject: the game's server physics step, game state only.

Avian 0.8.0 supplies the per-body floor (upstream #1035: `MIN_PAR_ITER_ENTITIES`, 4,096 matched
entities). The 0.7 per-body thresholds are not carried or re-measured. Every per-body loop is
serial under either at the game's recorded populations (MEASURED: awake bodies peak near 160 in
the scheduled collapse). Collider transform propagation uses the dispatch described below;
continuous collision's per-body floor remains unmeasured.

`prepare_contact_constraints` dispatches over colors as upstream. The 0.7 fork kept it serial
through 561 manifolds, a crossover MEASURED on that stack (Bevy 0.19, two compute threads); nothing
measured on this one supports carrying it.

## Collider transform propagation on the compute pool (`src/collision/collider/collider_transform/plugin.rs`): kept

Subject: the game's server physics step, game state only.

`propagate_collider_transforms` walks each root's hierarchy. Avian 0.7 dispatched every root to the
compute pool; 0.8.0 runs them serially below 4,096 roots, the floor of loops whose item is one
body's update. Here an item is a whole hierarchy (a construction's every element, a piece's
chunks), and the game's roots stay under the floor, so the pass was serial. The patch restores
0.7's dispatch (`par_iter_mut`). Roots own disjoint hierarchies (upstream's own safety argument),
so the dispatch changes no result.

MEASURED on the scheduled collapse (`scripts/perf/collapse.sh --target local --schedule
scripts/perf/match-collapse.json --rehearsal --tanks 30 --clients 1`, debug server, compute pool
10, one run each): the phase that holds the pass (`physics_prepare`) took 436.8 µs wall and
443.7 µs CPU per tick serial, 176.6 µs wall and 621.5 µs CPU dispatched; the 0.7 stack, 184.8 and
637.9. On a compute pool of two the 0.7 fork measured the pass level or faster than serial at the
pile (its patch record); not re-measured on two threads here.

Kept: the dispatch is one call inside an upstream system. 7 fork lines (MEASURED). No guard: no
result depends on it.

## Body size metrics once per body (`src/dynamics/rigid_body/body_size_metrics.rs`): kept

Subject: the game's server physics step, game state only.

`update_body_size_metrics` recomputes a body's `BodySizeMetrics` (the thickness and reach that
continuous collision reads) over all the body's colliders. Upstream does so once per changed
collider ("bodies may be visited multiple times"), so a body whose N colliders changed in one step
costs N² collider visits, each a scan of the collider's hull points. The tick a construction's
piece takes its chunks changes hundreds of colliders on one body (962 at a house's collapse).

The patch collects the stale bodies (a changed centre of mass, or any changed collider) in a set
and recomputes each once. A body owns its metrics and reads only its own colliders, so the stale
bodies run on the compute pool (`par_iter_many_unique_mut`, which takes only a set: a body visited
twice is unrepresentable); a step with no stale body dispatches nothing. A changed collider whose
body is despawned, disabled or no longer a rigid body is skipped, and a body with no collider is
never stale (Avian removes its `RigidBodyColliders` with the last one), as upstream. An enabled body
missing a component the pass reads panics naming the invariant; pristine 0.8.0's `if let Ok` skips
it unseen with the rest, and the patch adds only the refusal and the stale set's filters.
`BodySizeMetricUpdates` holds the count of bodies the last pass recomputed: test instrumentation,
read only by the guard, at one relaxed atomic add per recomputed body and one resource write per
step. It cannot be `cfg(test)` (the game's tests build this crate as a dependency), and a feature
switched on from the game's dev-dependencies would compile the fork and every crate above it in
two feature sets.

MEASURED on the scheduled collapse (the command above, one run each): the physics step's time
outside Avian's timed stages was 151 to 518 ms in the two ticks after each collapse and 2.35 s over
the ten worst ticks; with the patch the worst is 0.6 ms and the ten worst sum to 3.9 ms. The run's
worst tick fell from 599.9 to 82.7 ms (the 0.7 stack: 106.7 ms).

Kept: the loop is inside an upstream system. 80 fork lines (MEASURED).

Guard: `src/tests/avian_body_size_metrics.rs`, 4 tests. Mutants, each red (MEASURED): every body
the pass cannot read skipped, as upstream (the refusal does not panic); every such body refused
(the disabled body panics); the moved-centre filter without `RigidBodyColliders` (the body with no
collider panics).

## Standalone key write-back: dropped

The 0.7 fork wrote a collider's new `ColliderTreeProxyKey` back when the `Remove<ColliderOf>`
observer moved it to the standalone tree; without it the stale key let a despawned collider's
standalone proxy outlive it, pairing with everything it overlapped. Avian 0.8.0 writes the key
back itself (upstream #1062, `collider_tree/update.rs`, "Store the new proxy key").

Witness: `src/tests/avian_collider_of_removal.rs`, 2 of 2 green on unpatched 0.8.0 (MEASURED).
Record: `upstream/avian-collider-of-removal-orphan-proxy.md`.

## Moved-list removal by index: dropped

Avian 0.7.0 took a proxy out of a tree's moved list and out of `MovedProxies` by a scan from the
front, so a tick that rehomed N colliders compared on the order of N² entries; the 0.7 fork
indexed both lists. Avian 0.8.0 indexes both itself: `ColliderTree::moved_proxies` is a
`MovedProxyList` (positions by proxy index) and `MovedProxies` keeps each key's position in a map.

One difference stays, named here because it is a fifth outcome by the game's rule: the 0.7 fork
panicked when a listed proxy was marked moved again, and `MovedProxyList::push` declines it and
returns `false`, which upstream's callers ignore. The hazard the panic guarded, a second entry
outliving the proxy's removal, cannot arise in an indexed list.

Witness: `src/tests/avian_moved_lists.rs`, 3 of 3 green on unpatched 0.8.0 (MEASURED): both lists
equal the scanning form after every step of a tick of 2,000 rehomes and 1,000 toggles and of
20,000 generated operations, and a second mark lists the proxy once.

## Warm-start matching (`src/collision/narrow_phase/mod.rs`, `src/collision/narrow_phase/system_param.rs`): hook

Subject: the game's server physics, contacts between bodies, game state only.

Upstream's matcher has two defects the 0.7 fork patched in place. `ContactManifold::match_contacts`
reads two `PackedFeatureId::UNKNOWN` ids as one identity, so a new point takes the first previous
point's warm start at any distance, and its swapped-orientation distance test is not under the
unknown-feature condition. The narrow phase calls it once per previous manifold, so a later
manifold overwrites a closer predecessor from an earlier one.

The fork now calls the matcher through `narrow_phase::ContactMatcher`, a resource holding a function
of the pair's new manifolds, its previous manifolds and the matching distance, where upstream ran
its loop. Not a field of `NarrowPhaseConfig`, which derives `Reflect`, `PartialEq` and serde; not
game-side, because the previous manifolds are a local of the private per-pair closure. Its default
is upstream's loop, and upstream's `match_contacts` keeps its 0.8.0 signature and body. Upstream's
gate stays: the matcher runs only with `NarrowPhaseConfig::match_contacts` on and at most four new
manifolds, so a pair with more (a dense trimesh) starts cold, as upstream. 36 fork lines (MEASURED:
24 in `mod.rs`, 12 at the call site and the field).

The law is the game's (`avian_ext::contact_match::match_pair`, installed by `avian_ext::plugin`):
each new point takes the closest admissible point among all the pair's previous manifolds; known
ids admit the same ids in either order, UNKNOWN ids admit only anchors within the distance.

A recycled pair returns before matching and keeps last step's manifolds. The game runs with
recycling off (`recycle_distance: 0.0`, set where the game composes physics); turning it on stops
matching for every recycled pair, and the pair guard below goes red (MEASURED).

Guards: `src/tests/avian_warm_start.rs`, 3 tests. The two that rank predecessors are red with
upstream's matcher (MEASURED: 5.0 taken where 11.0 is closest). The third requires a cold start with
no admissible predecessor (an UNKNOWN point 1 m from its only one; a known point whose only one
holds other ids, its anchors coincident in either orientation), red under each of three mutants of
the law (MEASURED): UNKNOWN ids admitted at any distance, known ids admitted by distance, and known
ids admitted by distance in the swapped orientation only, upstream's. Local upstream drafts:
`upstream/avian-unknown-contact-feature-identity.md`,
`upstream/avian-nearest-warm-start-contact.md`.

## Stationary non-touching pair cache (`src/collision/narrow_phase/stationary_miss.rs`, `src/collision/narrow_phase/system_param.rs`, `src/collision/contact_types/mod.rs`, `src/collision/diagnostics.rs`): module

Subject: the game's server physics, contacts between bodies, game state only.

A pair of built-in colliders whose geometry query found no contact while both sides held still
keeps every input that query and its point pruning read, boxed on the pair (`ContactPair`'s
`pub(crate)` `stationary_miss`, skipped by serde). A later visit of the same non-touching, still pair
with equal inputs skips the query: the query is a pure function of them. Touching pairs, pairs a
hook rejected (the key is kept only when the geometry itself found no manifold) and custom collider
backends, whose `Context` may feed the query, always query. A side holds still when standalone, or
when its body is static, sleeping or at exactly zero velocity; a body the narrow phase reads as
absent is unknown, never still. The inputs: both colliders' poses, shape change ticks and owners,
the summed collision margin, the speculative distance (contact tolerance and speculative CCD
included), the relative linear velocity, both angular velocities, the collider offsets and rotated
centers of mass, and the step length.

`CollisionDiagnostics::{stationary_non_touching_pairs, stationary_non_touching_queries}` count the
step's still non-touching pairs visited and the queries they made, the query count beside the
query; the game's tick trace reads them.

Recycling (`recycle_distance > 0`) returns before the cache, so with it on a recycled miss is
counted neither as a pair nor as a query.

Module: the key, the tallies and their rules live in the fork-only `stationary_miss.rs` (78 lines);
not game-side, because the check runs inside the private per-pair closure before the geometry query.
74 lines in upstream files (MEASURED: 56 in `system_param.rs`, the key built inline from the
closure's locals; 12 in the diagnostics; 5 for the field; 1 for the module).

Guards: `src/tests/avian_narrow_phase.rs`, the seven `stationary_*` tests: reuse and per-step reset,
motion and touching pairs, each key input, a wake with and without motion, a new neighbour, a
child's motion and rehoming, a hook rejection. With the cache bypassed five fail (MEASURED); a key
kept through a hook rejection fails the hook test (MEASURED). The wake test also needs a sleeping
side's miss to stay active, which Avian 0.8.0's island sleep breaks (it moves every pair whose
other body rests to the sleeping set); see "Resting misses stay active", which keeps it. Count evidence and
reproduction: `upstream/avian-still-pairs.md`.

## Resting misses stay active (`src/collision/contact_types/contact_graph.rs`, `src/dynamics/solver/islands/sleeping.rs`): hook

Subject: the game's rigid-body rest, server physics, game state only.

Avian 0.8.0 (#1032) moves to the sleeping set every pair of a sleeping island whose other body
rests, misses included. A held miss skips the narrow phase, so it needs a second, hand-kept list of
the inputs a miss reads, and each input the list lacks is a contact found late. Three holes, each a
witness that fails under the transfer (MEASURED on this fork with the refusal removed): the held
update predates the sleeping step's motion; a sleeping body's collider reshaped; an impulse widens
a sleeper's speculative margin with no bound changed (for a body with `SpeculativeCcd`: 0.8.0's
speculative contacts are opt-in, and without them a velocity is no input of a miss).

`ContactGraph::sleep_entity_with`'s `should_sleep` now sees the pair's edge with the other body
(its signature: `Fn(&ContactEdge, Option<Entity>) -> bool`), and `SleepIslands` passes "touching,
and the other body rests". A miss stays active and the narrow phase reads its inputs every step,
the stationary-miss cache making that cheap. Touching pairs sleep as upstream. 12 fork lines
(MEASURED: 8 in the graph, 4 at the caller).

The saving forgone, measured on 0.7 with the misses held: the shipped server parked on a pile
(`scripts/perf/pile_park.py pile`) went from 430.9 to 335.8 µs of physics per tick (MEASURED,
2026-11-15, medians of 4 interleaved runs).

Guards, all green and all red with the transfer restored (MEASURED): in
`src/tests/avian_island_sleep.rs`, `an_impact_wakes_an_island_that_merges_again_before_the_solver`,
`a_miss_closed_by_the_last_awake_step_links_the_resting_islands` and
`an_impulse_on_a_sleeper_meets_its_neighbour_struck_in_the_same_step` (the middle one also needs
the wake rule, "Island contact waking"); in `src/tests/avian_narrow_phase.rs`,
`stationary_miss_retests_when_a_woken_body_moves`.

The guards run with recycling off, as the game does: a recycled pair returns before the narrow
phase's query, so a resting miss closed by a sleeper's last awake step is never found
(MEASURED: `recycled_count` 1, the islands left apart).

## Island contact waking: game-side

Subject: the game's rigid-body rest, server physics, game state only.

Avian 0.8.0 wakes every sleeping island a new contact links (`apply_contact_status_change`), a
resting body's new stationary support included. The game's rule: only an awake body wakes a resting
island. The game holds it in a pass on stock Avian (`avian_ext::island_wake`,
`link_resting_contacts` in `NarrowPhaseSystems::Last`, after the reduction pass): each queued
`StartedGeneratingConstraints` whose sides are each in a sleeping island or hold no island (a
standalone collider, or a body out of the solver without a node: static, disabled, or asleep since
its spawn), at least one in an island, leaves the queue; a body in the solver without a node has
lost state `IslandPlugin` requires and is refused (guard:
`an_awake_body_without_its_island_node_is_refused`). The pass links the contact with
`PhysicsIslands::add_contact`, merging the resting islands it bridges, and moves the pair to the
sleeping set with `ContactGraph::sleep_entity_with`, so the solver gets no constraint for it. The
island's next wake (`WakeIslands`) returns the pair and pushes its manifolds. A merge the pass makes
stands when a later entry of the same step wakes the merged island, which then holds the pair. Sleep
is read from the island (`PhysicsIsland::is_sleeping`, public), not the body's `Sleeping` marker.
Upstream composes without `IslandPlugin` too; the game's does not (no app in the tree installs the
seams without it), and `avian_ext::plugin` refuses such a composition as it is built, naming the
plugin (guard: `the_seams_refuse_a_composition_without_islands`).

A predicate inside `apply_contact_status_change` was not enough: by then the pair's manifolds are in
the constraint graph, and the flush holds the contact graph read-only, so the hook would also have
to pop them and transfer the pair. The pass does both before the flush, with no fork line.

0 fork lines. Row-5 check, on this fork with the pass off and on (MEASURED): off,
`stationary_contact_does_not_wake_a_resting_island` and
`a_miss_closed_by_the_last_awake_step_links_the_resting_islands` fail, and
`an_impact_wakes_an_island_that_merges_again_before_the_solver`,
`removing_sleeping_restarts_the_woken_bodys_rest_interval` and
`an_impulse_on_a_sleeper_meets_its_neighbour_struck_in_the_same_step` pass; on, all five pass.
Mutants, each red (MEASURED): an awake side counted as resting (the impact test), no transfer to
the sleeping set (the stationary, closed-miss and impulse tests), no island link (the closed-miss
test).

## Island rest history (`src/dynamics/solver/islands/sleeping.rs`): kept

Subject: the game's rigid-body rest, server physics, game state only.

Rest history belongs to each body. `WakeIslands` restarts the rest interval (`SleepTimer`) of the
bodies that were sleeping only: an island merged from an awake and a sleeping one is marked sleeping
(`merge_islands`), and upstream's wake zeroed the awake members' timers with the rest. The removal
hook of `Sleeping` restarts its own body's interval, which has lost the marker by the time its
queued wake runs. A linked body without `BodyIslandNode` or `SleepTimer` panics in the wake
("every linked island body retains BodyIslandNode and SleepTimer"), where upstream ended the walk
and left the island partly awake.

Kept: the timer reset and the walk sit inside an upstream command and hook. 23 fork lines
(MEASURED).

Guards: in `src/tests/avian_island_sleep.rs`,
`waking_a_merged_island_preserves_already_awake_rest_time` and
`a_linked_body_without_sleep_timer_is_refused`, both red on upstream 0.8.0 (MEASURED), and
`removing_sleeping_restarts_the_woken_bodys_rest_interval`, red without the hook's reset (MEASURED).
Record: `upstream/avian-island-rest-history.md`.

## Rest energy (`src/dynamics/solver/islands/sleeping.rs`, `src/dynamics/solver/islands/mod.rs`): hook

Subject: the game's rigid-body rest, server physics and scenery, game state only.

`update_sleeping_states` asks the `RestTest` resource whether each awake body rests this step,
passing the body's solver state, its `SleepThreshold`, the length unit and its computed mass,
angular inertia and rotation; passing advances the body's `SleepTimer`, failing restarts it. The
default is upstream's separate linear and angular test, moved unchanged into `velocity_rest`. The
game installs its law (`avian_ext::rest_test::at_rest`): upstream's test, or kinetic energy per mass
under the rest speed's, `v² + ω·I_world·ω/m < (SleepThreshold::linear · PhysicsLengthUnit)²`. The
law and its account are the 0.7 fork's.

A body's timer is its own, so the test runs per body on the compute pool, as on the 0.7 fork, from
the floor Avian 0.8.0 gives its other per-body loops (`MIN_PAR_ITER_ENTITIES`, 4,096 awake bodies;
the 0.7 fork's measured 1,500 is not carried). Each body hands its island and rest time to a
per-thread list; the islands' effects then commit in any order to one result: the wake bits are a
union, and the split candidate is the sleepiest island, a tie going to the lesser island id
(upstream's serial loop kept the first in query order). A body whose `SolverBodyIndex` names no
live `SolverBodies` slot panics ("every indexed awake body has a live SolverBodies slot"), where
upstream skipped it unseen, its timer and its island's wake vote with it: an index is inserted after
its slot is pushed and removed with it.

Kept as a hook: the test sits inside an upstream system. 143 fork lines (MEASURED: 137 in the
system's file, the loop body re-indented under the parallel pass, and 6 in the re-export).

Guards: in `src/tests/avian_island_sleep.rs`, the rest tests, all green. Without the game's law
installed, `rest_depends_on_energy_per_mass_and_spin_axis` and
`a_supported_bar_rests_through_periodic_one_tick_roll_pulses` fail; with the energy test alone,
`the_original_velocity_test_still_allows_rest` and
`a_long_bar_see_sawing_below_the_angular_limit_still_rests` fail (MEASURED). On the parallel pass,
`rest_on_the_compute_pool_commits_every_body` (4,097 bodies, run alone in a child process on a
compute pool of four threads, whatever pool its parent process holds) is red with a body's commit
dropped and with its timer held at zero (MEASURED).

## Compliant contact (`src/dynamics/solver/contact/normal_part.rs`): hook

Subject: the game's tank driving physics (the track belt against loose pieces), game state only.

`ContactNormalPart` gains `compliant: bool` and `spring: SoftnessCoefficients`, and
`solve_impulse` two arms for a compliant point. At positive separation it removes its accumulated
impulse, so it pushes only while its spring is compressed and never stops a body short of it. In
overlap it solves `spring` in the biased and the relaxing pass alike, uncapped by the overlap solve
speed, so its compression under a steady load is the spring's static deflection. A new part is
rigid (`compliant` false, `spring` a copy of `softness`).

The law and its writers are the game's (`avian_ext::compliance`): a writer sets a manifold's
`ContactCompliance` in the `Laws` resource, keyed by contact and manifold index, after the narrow
phase; the store is cleared at the start of every step. After `SolverSystems::PrepareContactConstraints`
the game finds each constraint of a manifold with a law by its `contact_id` and `manifold_index`,
subtracts the law's `rest` from each point's initial separation, and sets `compliant` and `spring`
at the point's effective mass and the substep. A point the law does not bear at the narrow phase
starts cold (its warm-start impulses zeroed) and solves inert. The 0.7 fork's
`ContactManifold::compliance` field, its narrow-phase reset and the substep argument of constraint
preparation are gone. A law of zero stiffness and damping solves inert at any effective mass, as a
point the law does not bear: each solve returns its accumulated impulse. The 0.7 fork reached the
same impulse through a bias of zero over zero, whose NaN its accumulated-impulse clamp read as zero;
the formula's coefficients with a finite bias in its place carry a cached impulse through every
solve of a point of zero effective mass (MEASURED: a locked box's cached 5 N·s kept, 60 N·s stored
as load over the step's solves).

Not game-side: no public field makes the relaxing pass soft, so the two arms must sit in the solve.
25 fork lines (MEASURED).

Guard: `src/tests/avian_compliant_contact.rs`, 3 of 3 green. Mutants, each red (MEASURED): `rest`
not subtracted (the resting test); a free point's warm start kept, the spring arm removed, the
speculative arm removed, or the law tested on overlap in place of the spring (the free-point test;
the spring arm also the resting test); the zero law given the formula's coefficients (the zero-law
test). Record: `upstream/avian-compliant-contact.md`.

## Body-pair contact reduction: game-side

Subject: the game's server physics, contacts between bodies, game state only.

Two bodies touching through many collider pairs (a broken piece's chunk hulls along a seam) carry
one manifold per hull pair. The game reduces them in a pass on stock Avian
(`avian_ext::contact_reduction`, `reduce_contacts` in `NarrowPhaseSystems::Last`): after the narrow
phase has regenerated, matched and queued every pair, before the solver drains the queue. It groups
the touching, constraint-generating, hook-free pairs of each two non-static `ReducibleContacts`
bodies, clusters their contacts by support normal and keeps, per normal, the vertices of the hull of
the points that bear and of the hull of the points that carried load last step, each to within
`HULL_TOLERANCE`. A group stays whole for the step when a point closes faster than the slower body's
`SleepThreshold::linear` (a landing), when it spans more than `MAX_NORMALS`, or when a point it would
remove overlaps deeper than the deepest kept point of its normal by more than the contact tolerance.
The law, its numbers and its measured account are the 0.7 fork's, moved unchanged but for the math
types (`f32`, `Vec3`, `Vec2`); the module states what the code holds.

The pass puts the kept manifolds back and corrects the solver's queue: a pair whose count it changed
has its change folded into Avian's queued `ManifoldCountChanged` (dropped at zero), or a new entry
where Avian queued none, never a second one; a pair starting to generate constraints needs none, the
flush reading its reduced count. Groups own disjoint pairs and reduce in parallel from eight groups
up (the 0.7 fork's floor, not re-measured); their effects commit in pair order. A taken pair loses
its recycling cache: with recycling on, a pair reduced to no manifold and then recycled keeps none
while its body moves less than the recycle distance (MEASURED: a 5 cm drop sank 7.2 cm through it
without the clear). `ReducibleContacts` requires a `SleepThreshold`, Avian's default when the body
sets none, so every marked body has the landing speed Avian's rest test reads.

Dropped from the 0.7 fork with nothing in their place: the `REDUCIBLE_CONTACTS` pair flag (the pass
queries the marker), the two status-site tolerances for a touching pair with no manifold, and the
island walk over contacts without constraint handles: 0.8.0 links a contact to its island when it
starts touching and unlinks it when it stops, whatever its manifold count. The six reduction
counters leave `CollisionDiagnostics` for the game's `ReductionCounts` resource, the last step's
totals.

0 fork lines. Guards: `src/tests/avian_contact_reduction.rs`, 24 tests, all green: the 21 of the
0.7 fork (15 of the law alone, and six world-level, among them a body held by its fifth corner, a
reduced body resting on its loaded supports and sleeping, and only marked non-static pairs
reducing), and three for the pass: one constraint per kept manifold through settling, a lift and a
drop, with enough groups for the parallel path (a mutant queueing beside Avian's entry, or none,
trips Avian's `push_manifold` assertion, MEASURED); a landing judged at the slower body's rest speed
(judged at the faster one it fails, MEASURED); and a recycled pair regenerated after reduction (the
sink above).

## Solver contact visits (`src/dynamics/solver/plugin.rs`, `src/dynamics/solver/diagnostics.rs`, `src/utils.rs`): kept

Subject: the game's server physics, contacts between bodies, game state only.

`SolverDiagnostics::{serial_contact_visits, parallel_contact_visits}` count the contact constraints
each contact pass dispatches (warm start, the biased solve and the relaxation in every substep, and
restitution once a step), serially or through parallel chunks; zero-restitution no-ops count. Each
step's diagnostics reset clears them. Overflow constraints are serial; a color counts as its
`par_for_each` dispatched it, which now returns that decision (the single-worker and short-slice
serial cases included) without changing its thresholds, iteration or chunking. No counter feeds a
physics decision. The game's work account reads them (`src/net/ram.rs`, `src/cost.rs`).

Kept: counters inside three upstream systems and the dispatch decision of an upstream helper; no
extension point sees either. 65 fork lines (MEASURED: 26 in the solver, 32 in the diagnostics, 7 in
`par_for_each`).

Guard: the game's `src/net/ram/avian_tests.rs`, which reads them through the game's work account.
`contact_visits_cover_each_pass_and_reset_each_step` asserts visits = constraints × (3 × substeps +
1) and, run alone in a child process on a compute pool of four threads, the classification: 4 floor
contacts, every visit serial; 128, every visit parallel. Red with every dispatch counted serial
(MEASURED: 2,432 serial where 2,432 parallel are due). The overflow star checks overflow's serial
visits.
