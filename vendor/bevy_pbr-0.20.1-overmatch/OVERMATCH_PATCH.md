# Overmatch Bevy PBR patch

This directory is the published `bevy_pbr` 0.20.1 crate with the patches below. Source changes
carry `OVERMATCH PATCH` tags. The game is a multiplayer tank video game; these patches affect
rendering only.

MEASURED with `diff -ru` over `src/` against pristine 0.20.1, including fork-only source: **357**
added + removed lines. Section totals: directional-shadow backport **0**, heightfield shadow
**310**, mesh-instance removals **47** (**10** visibility guard, **37** channel removal).

Produced from `https://static.crates.io/crates/bevy_pbr/bevy_pbr-0.20.1.crate`, whose sha256
`1138d6ed9bbcc43a4d83878062869c20d4292f1112b177acfc1a20cf6ae14f3e` matches the crates.io index's
`cksum`, by `tar -xzf bevy_pbr-0.20.1.crate -C vendor/bevy_pbr-0.20.1-overmatch --strip-components=1
--exclude='*/Cargo.lock' --exclude='*/.cargo_vcs_info.json'`. The published lockfile and VCS record
are omitted; neither is read for a path dependency.

## directional_shadow_factor: dropped

Upstream 0.20.1 supplies the field, its initialization and its use inside the shadow-receiver
block (`src/render/pbr_types.wesl`, `src/render/pbr_functions.wesl`). No backport remains.

## Heightfield shadow: re-ported

`src/heightfield_shadow.rs` owns the main-world field, extraction and GPU uniform. Every mesh view
binds the non-filterable field texture at group 0 binding 39 and its uniform at binding 40;
MEASURED pristine 0.20.1 ends its view bindings at 38. No binding index moved.

The WESL edits are `mesh_view_types.wesl` (uniform), `mesh_view_bindings.wesl` (bindings) and
`shadows.wesl` (receiver law). The receiver arithmetic, matching-light threshold and multiplication
inside the cascade envelope are unchanged from the 0.19.1 fork. The other two old shader edits
belonged to the now-upstream directional-shadow multiplier.

No resource, or an image not yet uploaded, is a legitimate absence: the uniform disables the
field and the texture binding holds the fallback image. Preparation runs after image preparation
and before mesh view bind groups; its queue writes must precede the reads of that field.
`GpuHeightfieldShadow::from_field` remains public for the game's headless receiver-law witnesses
in `src/terrain_shadow.rs`; their source paths now name these WESL modules.

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
