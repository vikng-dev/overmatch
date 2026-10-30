# Overmatch lightyear_avian3d patch

This directory is the published `lightyear_avian3d` 0.30.1 crate (extracted from the `.crate` whose
sha256 matches `Cargo.lock`) with the source change below, tagged `OVERMATCH PATCH` so the diff
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
