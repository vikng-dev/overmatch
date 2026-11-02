# Overmatch lightyear_avian3d patch

This directory is the published `lightyear_avian3d` 0.30.1 crate (extracted from the `.crate` whose
sha256 matches `Cargo.lock`) with the source changes below, tagged `OVERMATCH PATCH` so the diff
against pristine 0.30.1 is self-describing. (`Cargo.lock` and `.cargo_vcs_info.json` from the
published crate are omitted; neither is read for a path dependency.)

## Shared child-collider placement (`src/plugin.rs`)

`LightyearAvianPlugin::update_child_collider_position` runs after the fixed loop in Position
mode, and delegates to the patched Avian's exported `ChildColliderPoses` system parameter
(`vendor/avian3d-0.7.0-child-pose`). Avian's pre-physics pass and the game's post-physics walk
boundary use the same parameter. Each system instance retains its own change-detection boundary.

The parameter visits children with changed local transforms, poses or attachments, and the
attached colliders of bodies with changed poses, kinds or collider lists. Bevy still scans the
change ticks of matching entities; unchanged children incur no pose composition or writes. A
re-enabled entity is placed from the patched Avian's buffered `Reenabled` message, which every
instance reads through its own cursor at any frame-to-tick ratio. Manual `ColliderOf`
attachments do not require a Transform `Children` component on the parent.

Pose expressions are shared with Avian and component writes compare every float bit, including
signed zeros and NaN payloads. Overmatch replicates no child collider (tank roots carry
`DisableReplicateHierarchy`), so no wire traffic keys on the unchanged children's ticks.

Guards: `src/tests/avian_child_pose_churn.rs`, including unchanged-input witnesses, every changed
input, re-enabling and manual attachment. Inspection:
`.agents/scratch/perf-2026-11-01/server-tick.md`. Earlier local upstream drafts:
`upstream/avian-child-collider-pose-churn.md`, `upstream/lightyear-avian-child-pass-visits.md`.
