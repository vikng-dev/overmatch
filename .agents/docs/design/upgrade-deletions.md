# Upgrade deletion audit — #368

Subject: the multiplayer tank game's rendering, platform policy, physics and damage model;
game state only. Audited from `b571f369` on `lane/stale-versions`. Scratch records are untouched.
Registry paths below are relative to `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.

## Issue dispositions

| Item | Disposition | Pinned-source evidence |
|---|---|---|
| macOS activation revoke | Deleted, with its module and exclusive direct AppKit feature requests. Hidden windows and frame captures are unfocused before WinitPlugin builds. | `bevy_winit-0.20.1/src/lib.rs:106` derives launch activation from initial `Window.focused`; `src/system.rs:128` also gates creation-time focus. Winit's `app_state.rs:439` skips invisible windows in its activation workaround. |
| `MeshUniform` alignment | Deleted, including its Rust-size/offset guard. Other raw-upload layout guards remain. | `bevy_render-0.20.1/src/render_resource/buffer_vec.rs:713` allocates `UninitBufferVec` by `T::SHADER_SIZE`; `:566` serializes CPU values through Encase. `src/batching/gpu_preprocessing.rs:281` uses the former; `render_resource/gpu_array_buffer.rs:48` selects `BufferVec` for CPU storage uploads. |
| RUSTSEC-2026-0344 ignore | Already gone at reset (`ccb5c419`). No `deny.toml` change. | `bevy_image-0.20.1/Cargo.toml:156` requests ruzstd 0.9; `Cargo.lock` resolves 0.9.1. The old 0.8 exception is absent. |
| `bevy_reflect` fork | Already gone at reset (`ff62d411`). | `glam-0.33.12/src/features/impl_serde.rs:896` unconditionally supplies the aligned boolean-vector serde implementations; `bevy_reflect-0.20.1/src/impls/glam.rs:502` registers them. The manifest has no reflection patch. |
| Audited second glam | Already gone at reset (`d682316f`). | `Cargo.lock` has one glam package, 0.33.12. `deny.toml:54` bans every other version and `tests::determinism_deps` checks feature unification. |
| Current-version prose and rules | Updated or corrected after source review; historical findings retained. | Current pins now agree with `Cargo.toml` and `Cargo.lock`. ADR-0028 prescribes neither retired compatibility patch; its dated measurement remains attributed to the earlier graph. |
| Vendored prunes | Kept: neither summary ticks nor Replicon's idle scan replaces their work. | Detailed below. No Replicon vendor or manifest patch exists at reset. |

## Why the prunes remain

- **Moved AABBs:** `vendor/avian3d-0.8.0-overmatch/src/collider_tree/update.rs:879` filters by
  pose/shape changes before fetching the complete collider item. Bevy's summary ticks are opt-in
  (`bevy_ecs-0.20.1/src/component/mod.rs:503`), and its ordinary `Changed<T>` still checks each
  row (`src/query/filter.rs:1133`). Avian's `Position`, `Rotation` and `Collider` do not opt in.
- **Child poses:** stock `avian3d-0.8.0/src/collision/collider/collider_transform/plugin.rs:85`
  still writes every child's pose. Faster tick inspection cannot remove those writes or their
  change notifications. The game-side placement law and Lightyear adapter still avoid them.
- **Pairless layers:** `vendor/avian3d-0.8.0-overmatch/src/collision/broad_phase/bvh_broad_phase.rs:89`
  skips tree traversals for layers that cannot pair. This is geometry eligibility, not change detection.
- **Stationary misses and owner aggregation:** their keys include collision-law inputs and collider
  ownership, respectively. A table tick does not establish either unchanged geometry-query inputs
  or one aggregation per body; their fork inventories and guards still describe the pinned code.
- **PBR removals:** `vendor/bevy_pbr-0.20.1-overmatch/src/render/mesh.rs:2168` avoids removing
  nonexistent instances; `:2740` batches real removals at their consumer. Summary ticks do neither.
- **Replicon:** `bevy_replicon-0.45.5/src/server.rs:820` skips an idle archetype only when its scan
  is valid and it has no pending replication. `server/replication_query.rs:95` still scans entity
  ticks in `any_changed`. This code is already upstream; there is no local scan patch to delete.

## Other conclusions from checking old claims

- **Cast extent cap: deleted.** `parry3d-0.31.1/src/query/gjk/gjk.rs:760` refines the stagnation
  result from witness separation. The old error-versus-extent estimate no longer justifies an asset
  limit. The limit, its refusal/fixture and the cast-site scanner are gone; target-local casts remain.
- **Boundary margin: kept.** MEASURED on the pinned stack: removing it, while probing contacts at
  the actual face, makes `world::boundary::tests::hulls_on_a_shipped_edge_solid_get_its_face` fail
  with “the face dropped a resting hull.” All construction sweep tests passed in that experiment.
  Restoring the margin passes the complete boundary family. `src/world/boundary.rs` retains the
  margin; its old measurements are explicitly attributed to the old Parry release.
- **Contact failure handling: kept.** `parry3d-0.31.1/src/query/contact/contact_support_map_support_map.rs:66`
  still turns EPA failure into no contact, and the nonlinear caster still refines a near-zero hit
  at the start pose (`query/nonlinear_shape_cast/nonlinear_shape_cast_support_map_support_map.rs:224`).
  The construction sweep's unresolved/retained-witness cases still pass against those paths.
- **Clearance certificate: kept.** `gjk::closest_points` can still stop without certifying
  separation. `scatter::clearance::apart` checks the reported direction against actual support gaps.
- **Tree scheduling: corrected.** The pinned fork's `collider_tree/optimization.rs:288` submits
  one compute task; finalization can claim its jobs at `:343`. The old async-pool rationale was
  false. Its old timing comparison remains a historical measurement, not a forecast for this fork.
- **Unlanded fog design: corrected.** `map-surroundings.md` now names the current WESL target and
  describes the fog patch as planned. `pbr_functions.wesl:904` still samples the receiver's shadow;
  the current fork does not contain the proposed unshadowed-scattering patch.
- The issue comment's sleeping-increment compensation is already gone: fork
  `dynamics/integrator/mod.rs:274` builds increments only for `With<SolverBodyIndex>` and the clear
  checks the same membership. `ReducibleContacts` and stock hook defaults remain design choices;
  neither has an upstream replacement in the pinned source.

## Version-line census

MEASURED with `git grep -n -P` against `b571f369`: **257 matching lines in 102 files**.
Diff classification: **149 version/citation-only updates, 38 rewritten or deleted lines,
70 unchanged historical lines**. The final tree has **75 historical matches**: some corrected
comments now explicitly name the older release that supplied their measurement.

The search covers `src/`, `assets/`, `scripts/`, `vendor/*/OVERMATCH_PATCH.md`, `.agents/docs/`,
`.agents/AGENTS.md` and the simplification skill's pin rule; it excludes this report and scratch.
It matches engine/crate names followed by the old Bevy, Avian, Lightyear, Parry, Replicon, glam,
wgpu and egui versions, explicit old patch versions and old engine tags. A second comment-only
search covers unqualified 0.19/0.30 references in `sandbox.rs`, `net/harness.rs`, `net/sync_margin.rs`,
`net/tests/net_fire_release.rs` and `net/tests/net_input_buffer_wrap.rs`.

Retained records include superseded ADRs, dated capture/research/review findings, old benchmark
attributions and release-introduction statements. They are not claims that the project still pins
those releases. Current source references use the pinned source or named symbols, rather than old
line offsets.

The prose-only changes leave the pre-existing lens rows `load-24` (`settings/ui.rs`), `net-12`
(`net_fire_release.rs`) and `net-22` (`tank/servo.rs`) unchanged. They change no behavior in those
items. No wire type or protocol revision changed.

## Checks

Each deletion commit passed formatting, CI clippy and its targeted tests. The render/platform
removals also passed rebuilt-client, hidden Metal shader runs (MEASURED: 30 seconds each).
The Parry removal passed 48 sweep, boundary and citation checks after restoring the needed margin.
`cargo deny --locked check -W unmaintained` with cargo-deny 0.20.2 passed advisories, bans, licenses
and sources; existing unmaintained/yanked warnings remain. The macOS linker reported its existing
large unwind-section warning; it did not fail a gate. The final prose change passed formatting, CI clippy and 165 targeted checks.
