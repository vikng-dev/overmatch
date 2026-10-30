# Draft: child-collider pose writes mark every child collider changed every step

**LOCAL DRAFT — not filed.** Performance finding against `avian3d 0.7.0` and
`lightyear_avian3d 0.30.1` with Bevy 0.19.1, read in the published sources. One record for both
crates: each carries its own copy of the same system, and the cost disappears only when both stop.

## Source and consequence

`avian3d`'s `update_child_collider_position`
(`src/collision/collider/collider_transform/plugin.rs`, `PhysicsStepSystems::First`, every physics
step) and `lightyear_avian3d`'s `LightyearAvianPlugin::update_child_collider_position`
(`src/plugin.rs`, `RunFixedMainLoopSystems::AfterFixedMainLoop`, every frame in Position mode)
both assign `position.0 = …` and `*rotation = …` through `Mut` for every collider with a
`ColliderOf` and no `RigidBody`, whether or not the value changed. Every child collider of every
rigid body, static bodies included, is therefore marked changed every step.

`update_moved_collider_aabbs` (`src/collider_tree/update.rs`) skips a collider whose `Position`,
`Rotation` and shape are unchanged since `LastPhysicsTick`. Its own TODO (`:888`) expects that skip
to do the work; the writes above defeat it for every child collider, so each step recomputes the
AABB of every collider of every static compound body (a building's walls, a tree's segments), none
of which moves.

## Evidence

Overmatch's dedicated server, 30 tanks on its product map, whose buildings and trees are static
bodies with one child collider per wall element or trunk segment (~13.8k map entities, MEASURED),
traced per system (MEASURED 2026-10-31, M4, `bevy/trace_chrome`):
the AABB update's parallel batches cost 1753 µs per tick and the system 638 µs, against 1034 and
425 µs at 2 tanks, so the cost follows the collider count, not motion. With both systems patched
as below, the same 30-tank server A/B/B/A on the stats line (MEASURED 2026-10-31, M4, release build,
4 × 60 s windows, both patched runs below both stock runs on every row): physics step p50 904 →
618 µs (−32 %), tick p50 4100 → 3922 µs (−4.4 %), process CPU 61.3 → 53.4 % of one core.

## Proposed upstream change

Compute the same pose with the same expressions and assign only where the value differs in bits
(`to_bits` per component, so a `-0.0` never replaces a `0.0` the unconditional write would have
stored, and NaN payloads compare as stored). The stored values stay bit-identical; only the change
ticks of unchanged values differ. Readers of those ticks in `avian3d`:

- `update_moved_collider_aabbs`: a pure function of pose and shape, so a skipped recompute
  reproduces the AABB already stored.
- `wake_on_changed`: reads only entities with a `BodyIslandNode`, which avian gives to a
  `SolverBody` or to a dynamic or kinematic `RigidBody`. The systems above exclude `RigidBody`.
- `position_to_transform`: filters on `Or<(With<RigidBody>, With<ApplyPosToTransform>)>`, so a
  child collider reaches it only if it carries `ApplyPosToTransform`. Avian never inserts that
  marker. `lightyear_avian3d` adds it only to an `Interpolated` non-rigid entity whose `ColliderOf`
  is absent or names itself (`add_apply_pos_to_transform`). It removes it from any child collider
  that gains it (`remove_apply_pos_to_transform_from_child_collider`). A child collider's
  `Transform` holds its local offset, and writing its world pose there would apply the hierarchy
  twice. So no child collider reaches the system in a lightyear app. An application that marks
  one itself gets its `Transform` rewritten only on the steps where the child's pose bits change.

A test would pin that a child of an unmoved static body keeps its `Position` change tick across a
step, and that a moved body's child still updates.

Cheaper still, and a separate decision: skip a child whose body pose and `ColliderTransform` are
unchanged since the system's last run. That saves the iteration too, but must also notice a direct
write to the child's own `Position`.

## Workaround

Overmatch vendors both crates with the bit-compare patch: `vendor/avian3d-0.7.0-child-pose` and
`vendor/lightyear_avian3d-0.30.1-child-pose` (`OVERMATCH_PATCH.md` in each). Removal condition:
both upstreams compare before writing.

## What fixing this unlocks for us

Drop both vendored crates. Nothing else: the patch is the whole workaround.
