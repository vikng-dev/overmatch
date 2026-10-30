# avian3d 0.7: a body that leaves the narrow phase's body query keeps its pairs' constraint handles, and gets a second set when it returns

**Target:** github.com/avianphysics/avian · avian3d 0.7.0, and `main` by source reading · **Severity
for us:** HIGH before the workaround: a client panic in `prepare_contact_constraints`
(overmatch #183) · **Status:** LOCAL DRAFT, not filed.

## Suggested title

`prepare_contact_constraints` panics with "index out of bounds" after a touching body briefly
leaves the narrow phase's body query

## Summary

A contact pair generates constraints only while both colliders' bodies match the narrow phase's
body query (`Query<RigidBodyQuery, Without<RigidBodyDisabled>>`, whose `RigidBodyQuery` needs
`Position`, `Rotation`, the velocities and the computed mass properties). When one body stops
matching it for a step, the pair's `GENERATE_CONSTRAINTS` flag clears but nothing removes the pair's
manifold handles from the `ConstraintGraph`. When the body matches again, the pair re-enters through
`STARTED_GENERATING_CONSTRAINTS`, which pushes one handle per manifold on top of the handles it kept.
The solver then indexes the pair's manifolds with a handle past the end.

## Mechanism (avian3d 0.7.0)

- `collision/narrow_phase/system_param.rs:587-602` (`update_contacts`): `is_disabled` is true when
  either collider's body is not in `body_query`; the flag is set to `!is_disabled`, and a
  false→true flip sets `STARTED_GENERATING_CONSTRAINTS` and the status bit. A true→false flip sets
  no status bit and records nothing.
- `system_param.rs:326-340` (`update`): the `STARTED_GENERATING_CONSTRAINTS` branch pushes
  `contact_pair.manifolds.len()` handles without looking at `contact_edge.constraint_handles`. If
  the islands plugin is on, it also calls `PhysicsIslands::add_contact` for a contact that is still
  linked, which trips `debug_assert!(contact.island.is_none())`
  (`dynamics/solver/islands/mod.rs:518`) in a debug build.
- Every pop of a pair's handles is gated on `generates_constraints()`: the disjoint-AABB branch
  (`system_param.rs:184-196`), the stopped-touching branch (`:295-306`) and the shrinking-manifold
  branch (`:368-387`). A pair that stops touching, or loses a manifold, while its flag is clear
  keeps handles it no longer has manifolds for.
- `dynamics/solver/plugin.rs:398` indexes `contact_pair.manifolds[handle.manifold_index]` before
  the `generates_constraints()` check at `:400`, so a stale handle panics even on a pair that would
  have been skipped.

Two messages follow from one gap, both seen in our client:

- `index out of bounds: the len is 1 but the index is 1`: the body returns while the pair still
  touches with one manifold; the re-add pushes handle 1 beside the kept handle 0. The
  0.7.0-alpha.2 client's crash.
- `index out of bounds: the len is 0 but the index is 0`: the pair stops touching while the flag is
  clear, so its handle survives with no manifold. Seen in a local reproduction, with a probe that
  logged, the step before, four pairs at `handles 1 != expected 0`, flags `TOUCHING` only.

## How a body leaves the query: lightyear's interpolation

`lightyear_interpolation` 0.30.1, `interpolate.rs:289-306` (`queue_history_presence`, called at
`:157` and `:237`) removes an interpolated component from the entity when the interpolation cursor
has no confirmed state at or before it, and inserts it again once it has one. A resync of the
interpolation timeline moves the cursor back past the oldest kept sample, so every interpolated
`Position` disappears for a frame. On our client, every replica hull is a `Static` body with an
interpolated `Position`. MEASURED (local run, 120 ms latency and 60 ms jitter on the receive path,
200 server-simulated fragments): 3 ms after the client logged an interpolation resync, every one of
30 replica hulls lost `Position`; the Edgegap run that crashed logged 48 resyncs in about 60 s. A
free turret body lying on a hull was the touching pair.

This is lightyear behaving as designed. The defect is that avian cannot survive a body leaving its
query, whoever removes the component; `RigidBodyDisabled` takes the same route out of the query
and has observers that clean up, but a removed `Position` has none.

## Minimal reproduction

Pinned in this repository as an inverted test, `src/tests/avian_body_query_gap.rs`. It passes
while the defect stands. In outline (default plugins without `IslandPlugin`/
`IslandSleepingPlugin`, one 64 Hz step per update):

```rust
let ground = world.spawn((RigidBody::Static, Position(Vec3::new(0.0, -0.5, 0.0)))).id();
// The collider is the body's child, so it stays queryable while the body's Position is gone.
world.spawn((ChildOf(ground), Transform::IDENTITY, Collider::cuboid(4.0, 1.0, 4.0)));
world.spawn((RigidBody::Kinematic, Position(Vec3::new(0.0, 0.49, 0.0)), Collider::cuboid(1.0, 1.0, 1.0)));
for _ in 0..4 { app.update(); }                      // touching: one manifold, one handle
let pose = *world.get::<Position>(ground).unwrap();
world.entity_mut(ground).remove::<Position>();
app.update();                                         // flag clears, the handle stays
world.entity_mut(ground).insert(pose);
app.update();                                         // a second handle: plugin.rs:398 panics
```

With the islands plugin on, a debug build stops one call earlier at `islands/mod.rs:518`; a release
build reaches the solver panic.

## On `main`

By source reading at `main` (v0.8.0; newest commit touching the narrow phase `a01e17db2`,
2026-10-20), not run: the mechanism stands. `system_param.rs:709-724` still clears
`GENERATE_CONSTRAINTS` on `is_disabled` without queuing
`ContactStatusChange::StoppedGeneratingConstraints`; the solver's handler for
`StartedGeneratingConstraints` (`dynamics/solver/plugin.rs:293-301`) pushes `manifolds.len()`
handles unconditionally; `plugin.rs:698-700` still indexes before checking the flag.

Related, not the same: avian#876 (closed in 0.8 by #1087, `manifold_count_change` overflowing
`i16`) reports this panic line from high-speed trimesh contacts; one comment there hits it on
inserting `RigidBodyDisabled`. #1068 clears `STARTED_GENERATING_CONSTRAINTS` when a collision
starts, which closes a different path to a double push.

## Suggested fix

1. In `update_contacts`, treat a true→false flip of `GENERATE_CONSTRAINTS` as a status change: set a
   stopped-generating flag and the status bit, and in `update` pop the pair's handles and unlink
   its island (on `main`: queue `StoppedGeneratingConstraints`). This mirrors what the
   `RigidBodyDisabled` and `Sensor` observers already do.
2. Make the re-add idempotent: push only up to `manifolds.len()` handles in total, or assert the
   pair holds none.
3. In `prepare_contact_constraints`, check `generates_constraints()` before indexing, and index with
   `get` so a bookkeeping slip cannot become a panic in the solver.

## Our workaround and its removal condition

The net client holds only static bodies: the client's launched turret, its one non-static body, is
now `RigidBody::Static` and moved by writing its pose (`apply_launched_turret_pose` in
`src/net/protocol.rs`). Avian's broad phase never pairs two static bodies, so no pair on the client
generates constraints and the constraint graph stays empty whatever lightyear removes.
`src/net/protocol.rs`'s `a_launched_turret_on_a_replica_hull_holds_no_contact_constraint` pins it.

Remove the rule when `src/tests/avian_body_query_gap.rs` fails: that failure means avian survives
the gap.

## What fixing this unlocks for us

Little today: the client wants no contact dynamics. It would let a client hold a non-static body
again, for instance client-side presentation debris near interpolated hulls, without first proving
that no interpolated body it can touch ever loses `Position`.
