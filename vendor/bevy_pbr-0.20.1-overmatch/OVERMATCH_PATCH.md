# Overmatch Bevy PBR patch

This directory is the published `bevy_pbr` 0.20.1 crate with the patches below. Source changes
carry `OVERMATCH PATCH` tags. The game is a multiplayer tank video game; these patches affect
rendering only.

Produced from `https://static.crates.io/crates/bevy_pbr/bevy_pbr-0.20.1.crate`, whose sha256
`1138d6ed9bbcc43a4d83878062869c20d4292f1112b177acfc1a20cf6ae14f3e` matches the crates.io index's
`cksum`, by `tar -xzf bevy_pbr-0.20.1.crate -C vendor/bevy_pbr-0.20.1-overmatch --strip-components=1
--exclude='*/Cargo.lock' --exclude='*/.cargo_vcs_info.json'`. The published lockfile and VCS record
are omitted; neither is read for a path dependency.

## directional_shadow_factor: dropped

Upstream 0.20.1 supplies the field, its initialization and its use inside the shadow-receiver
block (`src/render/pbr_types.wesl`, `src/render/pbr_functions.wesl`). No backport remains.

## Heightfield shadow: dropped

The game binds the terrain's baked shadow on its own materials and multiplies it into upstream's
`directional_shadow_factor` (`src/terrain_shadow.rs`, `assets/shaders/terrain_shadow.wesl`); no
view binding, uniform or receiver law remains here. MEASURED against the pristine crate with
`diff -ruN` (excluding this file, `Cargo.lock` and `.cargo_vcs_info.json`): 357 changed lines
(+335, −22) across seven files before the drop, 47 (+27, −20) in `src/render/mesh.rs` alone after.

## Mesh-instance removals: re-carried

`src/render/mesh.rs` keeps the worker queues' removals until the consumer applies them, after all
prepared instances and before re-extraction requests, instead of sending one channel message per
entity. Removal effects commute across entities; channel arrival never supplied an order.

An invisible mesh queues a removal only if the previous collection left an instance for it.
Every input slot and GPU-culled entry belongs to such an instance; extraction adds an instance
this frame only for a changed mesh. Removing `Mesh3d` or despawning still queues unconditionally.

Witnesses: `src/tests/bevy_mesh_removal_lifecycle.rs`, run against the real `PbrPlugin` on an adapter
at integration. The queue iterator used by the lifecycle tripwire remains available under the
same upstream interface.
