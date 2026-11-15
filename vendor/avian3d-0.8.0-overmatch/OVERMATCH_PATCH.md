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
of `src/` against the published crate.

Guards run in `vendor/avian3d-0.8.0-guards`, a crate beside this one that builds the fork with the
game's `src/avian_ext/` and the game's `src/tests/avian_*.rs` included by path, without the game
crate (`CARGO_TARGET_DIR=<bay>/target/avian-guards scripts/serial-cargo test --profile test
--manifest-path vendor/avian3d-0.8.0-guards/Cargo.toml`). It is deleted when the guards run as the
game's own tests again.

## Child collider poses (`src/collision/collider/collider_transform/plugin.rs`): hook

`ColliderTransformPlugin` no longer registers `update_child_collider_position`, which assigned every
child collider's `Position` and `Rotation` through `Mut` every step, changed or not: every child
collider of every body, static bodies included, was marked changed every step, and
`update_moved_collider_aabbs`, which skips a collider whose pose and shape ticks did not move,
recomputed all their bounds. On the product map that is every building element and tree segment,
none of which moves. The function stays, unregistered, so the edit is the registration alone
(13 fork lines, MEASURED).

The placement law is the game's (`avian_ext::child_poses`): `place_child_colliders`, which the game
runs in `PhysicsStepSystems::First` (the slot upstream used) and at the frame boundary where
lightyear_avian ran its own child pass. It composes the same pose and writes a component only where
its bits differ, visits only children whose offset, own pose, attachment or body changed, and reads
`Reenabled` messages for re-enabled entities, whose re-enable moves no tick.
`ColliderTransformPlugin` stays enabled: it also propagates `ColliderTransform` and adds the
`pub(crate)` `AncestorMarkerPlugin`, which a disabled plugin would leave to the game.

Guard: `src/tests/avian_child_pose_churn.rs`. Record: `upstream/avian-child-collider-pose-churn.md`.

## Attachment refusal: game-side

`ColliderOf::on_insert` returns without listing the collider in its body's `RigidBodyColliders`
when the collider or the body has no `GlobalTransform`. Every child placement boundary reaches a
moved body's colliders through that list, so such a child stays at its birth pose. Avian's
`PhysicsTransformPlugin` requires `Transform` through `Position`; the game's networked composition
disables that plugin, so there a body spawned without `Transform` takes this path silently.

The game refuses it in an observer on `Insert<ColliderOf>` (`avian_ext::attachment`), which runs
after the hook on the same insertion and panics naming both entities and the fix. The 0.7 fork
panicked inside the hook. Cost of the observer, MEASURED in the guard crate's unoptimized test
profile, 10 000 attachments spawned onto 2 000 static bodies, three runs each: 305 to 310 ms
without it, 315 to 316 ms with it, about 2%; the hook form is not worth its fork lines.

Guard: `src/tests/avian_child_pose_churn.rs`
(`a_collider_attached_to_a_body_without_a_global_transform_is_refused`; stock 0.8.0 does not
panic, MEASURED).

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
sensor. Guard: `src/tests/avian_prepared_mass.rs` (it fails with the scale invalidation removed,
MEASURED). Local upstream draft: `upstream/avian-prepared-collider-mass.md`.

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

## Sleeping bodies' velocity increments: game-side

Subject: the game's rigid-body rest, server physics, game state only.

Avian 0.8.0 still builds every body's world-frame velocity increment each step (constant forces
and accelerations, gravity, the scaling by the substep in `pre_process_velocity_increments`) and
clears only bodies with a `SolverBodyIndex`. A sleeping body has none and keeps its
`VelocityIntegrationData`, so its increment compounds while it sleeps and its first step awake
gains about `N·g·h²/(1 − h)` extra (DERIVED; `h` the substep length in s, `N` the substeps).
MEASURED in the guard: −0.15368 m/s gained where gravity gives −0.15328.

The 0.7 fork filtered the increment builders to solver bodies. 0.8 needs no fork line:
`avian_ext::sleeping_increments` holds the non-zero increments of bodies out of the solver before
`ForceSystems::ApplyConstantForces` and, in `IntegrationSystems::ClearVelocityIncrements`, clears
those bodies and gives back what it held. A body out of the solver therefore ends each step with
the increment it began it with, as under the 0.7 patch: a force written to a sleeper (the waking
`Forces` call wakes the island only in `PhysicsStepSystems::Sleeping`, after the solve) acts,
unscaled, in its first step awake. A clear alone loses that force: MEASURED, the woken body gained
0 where the force gives 2.44 m/s; stock Avian gives it 0.0064.

Guards: `src/tests/avian_island_sleep.rs` (`a_woken_body_takes_only_its_first_steps_gravity`,
`a_force_on_a_sleeper_acts_in_its_first_step_awake`). Local upstream draft:
`upstream/avian-sleeping-velocity-increments.md`.

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
