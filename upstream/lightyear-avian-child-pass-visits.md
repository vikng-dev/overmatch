# Draft: lightyear_avian's child-collider pass reads every child collider every frame

**LOCAL DRAFT — not filed.** Performance finding against `lightyear_avian3d 0.30.1` with
`avian3d 0.7.0` and Bevy 0.19.1, read in the published sources. It follows
[avian-child-collider-pose-churn.md](avian-child-collider-pose-churn.md), whose bit-compare patch
it assumes.

## Source and consequence

`LightyearAvianPlugin::update_child_collider_position` (`src/plugin.rs`,
`RunFixedMainLoopSystems::AfterFixedMainLoop`, every frame in Position mode) iterates every
collider with a `ColliderOf` and no `RigidBody`, looks its body up, recomputes its pose from the
body's `Position` and `Rotation` and its own `ColliderTransform`, and writes it. Once the write is
bit-compared (the record above), the pass changes nothing for a child whose body stands, yet it
still reads every child of every body every frame, static bodies' included, and the frame rate of a
headless server runs it several times per tick.

## Evidence

Overmatch's dedicated server, 30 tanks on its product map, whose buildings and trees are static
bodies with one child collider per wall element or trunk segment (~13.8k map entities, MEASURED),
runs the pass 2.55 times per fixed tick (MEASURED): 276 µs per tick traced before the bit-compare
patch (MEASURED 2026-10-31, M4, `bevy/trace_chrome`). With it, the pass still cost 372 µs per tick
traced at 3.04 runs per tick (MEASURED 2026-10-31, M4, 30 bots and no client), to find nothing moved
on almost every child. The change below is priced separately.

## Proposed upstream change

Visit only the children whose pose can differ from the one the pass last left. The pose is a
function of the child's `ColliderTransform` and its body's `Position` and `Rotation`, written only
where its bits change, so a child whose inputs, own pose and attachment are unchanged since the
system's last run already holds the bits the pass would compute. Visit:

- each child whose `ColliderTransform`, `Position`, `Rotation` or `ColliderOf` changed
  (`Or<(Changed<…>, …)>` on the child query);
- each collider of a body whose `Position`, `Rotation`, `RigidBody` or `Children` changed (a
  `Changed` body query, walking `RigidBodyColliders`);
- every child, after the removal of any disabling component (`DefaultQueryFilters::disabling_ids`,
  read through `RemovedComponentMessages`): a re-enabled entity matches the queries again without
  any of those ticks moving.

An entity that loses its `RigidBody` loses its `ColliderOf` with it (avian's `on_body_removed`), so
it re-enters the child query only through a `ColliderOf` insertion, which the filter sees. The pass
relies on every writer of those components marking them changed; a `bypass_change_detection` write
is not seen. Tests would pin that a child whose inputs stood keeps a pose planted without a change
tick, and that each input change, and a re-enable after the body moved, carries the child again.

## Workaround

Overmatch vendors the crate with this patch on top of the bit-compare one:
`vendor/lightyear_avian3d-0.30.1-child-pose` (`OVERMATCH_PATCH.md`), guarded by
`src/tests/avian_child_pose_churn.rs`. Removal condition: upstream visits only changed children.

## What fixing this unlocks for us

Drop the vendored crate, together with the bit-compare record's fix. Nothing else.
