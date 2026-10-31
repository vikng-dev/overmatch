# Overmatch lightyear_avian3d patch

This directory is the published `lightyear_avian3d` 0.30.1 crate (extracted from the `.crate` whose
sha256 matches `Cargo.lock`) with the source changes below, tagged `OVERMATCH PATCH` so the diff
against pristine 0.30.1 is self-describing. (`Cargo.lock` and `.cargo_vcs_info.json` from the
published crate are omitted; neither is read for a path dependency.)

## Child-collider pose writes (`src/plugin.rs`)

`LightyearAvianPlugin::update_child_collider_position` runs every frame
(`RunFixedMainLoopSystems::AfterFixedMainLoop`, Position mode) and assigned every child collider's
`Position` and `Rotation` through `Mut`, whether or not the value changed. Avian's
`update_moved_collider_aabbs` skips a collider whose pose and shape are unchanged since the last
physics tick; these writes land after that tick, so every child collider of every rigid body,
static bodies included, had its AABB recomputed every step.

The patch computes the same pose with the same expressions and assigns `Position` and `Rotation`
only where the new value differs in bits (`to_bits` per component, so `-0.0` against `0.0` and NaN
payloads count as different). The stored values are bit-identical to the unpatched system's; only
the change ticks of unchanged values differ. Overmatch replicates no child collider (every tank root
carries `DisableReplicateHierarchy`), so no wire traffic keys on them.

Avian's own copy of the system is patched the same way (`vendor/avian3d-0.7.0-child-pose`); the
saving needs both. The defect and the proposed upstream fix are recorded in
`upstream/avian-child-collider-pose-churn.md`.

## Child pass visits only changed children (`src/plugin.rs`)

The same system then still read every child collider every frame, 2.55 frames per tick on the
dedicated server (MEASURED, 2026-10-31 profile), to find that almost none had moved. It now visits
only the children whose pose can differ from the one it last left: each child whose
`ColliderTransform`, `Position`, `Rotation` or `ColliderOf` changed since the system's last run, and
each collider of a body whose `Position`, `Rotation`, `RigidBody` or `Children` changed (both through
`Changed` filters; an insertion counts). After the removal of any disabling component
(`DefaultQueryFilters`) it visits every child, since a re-enabled entity matches again without one
of those ticks moving. The pose expressions and the bitwise write are unchanged (`child_pose::write`).

Why a skipped child is exact: its pose is a function of its `ColliderTransform` and its body's
`Position` and `Rotation`, and the last run that visited it left the bits that function gives. With
none of those inputs, its own pose or its attachment changed since, the function gives the same bits
again. An entity that loses its `RigidBody` loses its `ColliderOf` with it (Avian's `on_body_removed`),
so it re-enters the query only through a `ColliderOf` insertion. Relies on every writer of those
components marking them changed: no writer in Avian, lightyear, bevy_replicon or Overmatch uses
`bypass_change_detection` on them (searched 2026-10-31).

Guard: `src/tests/avian_child_pose_churn.rs` (a child whose inputs stood keeps a pose planted without
a change tick; each input change, and a re-enable after the body moved, carries the child again).
Record: `upstream/lightyear-avian-child-pass-visits.md`.
