# Overmatch lightyear_avian3d patch

This directory is the published `lightyear_avian3d` 0.31.0 crate with the child-pose registration
change below, marked `OVERMATCH PATCH` at its site.

Produced from `https://static.crates.io/crates/lightyear_avian3d/lightyear_avian3d-0.31.0.crate`,
whose sha256 `d0ea43838ce698bd3b21b0298b74a6838b3f1942052f4ccea2489413d766c739` matches the
crates.io index's `cksum`, by `tar -xzf lightyear_avian3d-0.31.0.crate -C
vendor/lightyear_avian3d-0.31.0-child-pose --strip-components=1 --exclude='*/Cargo.lock'
--exclude='*/.cargo_vcs_info.json'`. The published lockfile and VCS record are omitted; neither is
read for a path dependency.

## Game-owned child-collider placement (`src/plugin.rs`)

Position mode does not register `LightyearAvianPlugin::update_child_collider_position` in
`RunFixedMainLoopSystems::AfterFixedMainLoop`. The game registers
`avian_ext::child_poses::place_child_colliders` in that slot. Each system instance retains its own
change-detection boundary; unchanged child colliders receive no pose writes.

This crate names no fork-only Avian API. `rollback.rs` is pristine: Avian 0.8 owns its moved-proxy
list and the fork adds no fields to the trees or that list. Transform mode's placement remains
upstream's; the game selects Position mode.

Witnesses in the game: `lightyear_position_mode_leaves_the_frame_boundary_to_the_game` in
`src/tests/avian_child_pose_churn.rs`, which mounts this plugin as the game composes it and fails
with the registration restored (MEASURED); the rest of that file, for the game's pass; and
`src/ballistics/recording_tests.rs`.
Standalone validation uses `check` (default 3d/parry-f32 features) and a command-line
`patch.crates-io.avian3d.path` pointing at the trunk's fork.
