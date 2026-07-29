# Simplification backlog

Surveyed on 2026-07-29 against `chore/simplify`, after MEASURED four existing branch commits.
The ranking is by code-quality value: first the duplicated fact whose drift would make a safety
check lie, then other duplicated production knowledge, then test-only overlap. Change sizes are
DERIVED estimates from the current source; they are not promises or line-count targets.

The exclusions supplied for this run remain binding. An entry that crosses one is recorded so a
future run does not have to rediscover it, but is not currently applicable. No entry proposes a
wire-surface change, a stored-to-derived rollback change, a new dependency, or a lint-policy change.

## Ranked backlog

### 1. DONE IN THIS RUN — make the bake verifier use the sim's canonical pose composition

- Files and symbols: `src/bake.rs::shadow_compare_on_instance_ready`,
  `src/bake.rs::compose_scene_pose`, and `src/tank/model.rs::rig_world_pose`.
- What was wrong: duplicated knowledge. `compose_scene_pose` was a verbatim identity-root copy of
  `rig_world_pose`, even though the shadow verifier exists to prove that extracted poses equal the
  poses the sim consumes. The algorithms could drift and let the verifier validate a different
  composition from the live sim.
- Estimated change: DERIVED about 20–30 source lines, localized to `src/bake.rs`.
- Behaviour-preserving confidence: high. The replacement passes the same entity, root, parent query,
  and local-transform query to the same loop, with the duplicate's `Vec3::ZERO` and
  `Quat::IDENTITY` supplied as the canonical function's root pose.
- Applicability / C2: safely applicable; no owner decision. This deletes a duplicate behind an
  already-established interface and introduces no pattern.

### 2. Reuse the existing glTF-node matrix conversion in the other shipped-asset checks

- Files and symbols: `src/track/marker_model.rs::node_matrix`,
  `src/track/link_view.rs::the_shipped_template_is_authored_at_unit_scale`, and
  `src/track/gear_phase.rs::glb_sprocket_tip`.
- What is wrong: duplicated knowledge. All sites spell the same mapping from
  `gltf::scene::Transform::{Matrix, Decomposed}` to `Mat4`, including the same
  scale/rotation/translation order. `marker_model` already owns a production helper; the tests
  repeat it inline.
- Estimated change: DERIVED about 20–30 source lines deleted across the files.
- Behaviour-preserving confidence: high. Widen `node_matrix` only to `pub(super)` and call that exact
  implementation; do not route through `Transform::from_matrix`, which would add a decomposition.
- Applicability / C2: safely applicable; no owner decision and no new pattern.

### 3. Give the GPU-less `DefaultPlugins` surgery one crate-owned implementation

- Files and symbols: `src/bitprobe.rs::build_app`, `src/cost.rs::headless_sim`,
  `src/headless_test.rs::headless_shell`, and `src/net/server.rs::run`; the same block also appears
  in the currently excluded `src/net/client.rs` and `src/settings/ui.rs`.
- What is wrong: duplicated knowledge. Each root independently disables the render backend, primary
  window, and winit runner with the same `DefaultPlugins` edits. A Bevy upgrade currently requires
  coordinated edits in several composition roots. The KTX2/ASTC workaround is a separate concern
  and must remain only at the roots that deliberately install it.
- Estimated change: DERIVED about 60–90 source lines once every occurrence can move together.
- Behaviour-preserving confidence: high if a crate-owned function performs only the current plugin
  edits and each caller retains its clock, runner, physics, and image-workaround choices.
- Applicability / C2: not applicable in this run because occurrences are excluded. No C2 owner
  decision is required. Under C2b, the shared composition-root helper is a new pattern and should be
  introduced visibly in its own change, with every current occurrence migrated together.

### 4. Share collision-proxy construction between the game spawn and the track sandbox

- Files and symbols: `src/tank/spawn.rs::insert_collision_proxies`,
  `src/track_sandbox/mod.rs::build_rig`, and `src/bake.rs::MeshGeometry`.
- What is wrong: duplicated knowledge. Both paths collect the same primitive `POSITION` bytes into
  `Vec3` and pass them to the pinned avian3d `Collider::convex_hull`; the sandbox comment explicitly
  says identical construction is the point because otherwise its playtest can lie about the
  shipped tank.
- Estimated change: DERIVED about 15–30 source lines, likely as a fallible method on
  `MeshGeometry`; parenting, transforms, layers, friction, and caller-specific panic context stay
  at the callers.
- Behaviour-preserving confidence: high if the helper returns the same `Option<Collider>` from the
  existing call and callers keep their present failure messages.
- Applicability / C2: safely applicable; no owner decision. The seam has MEASURED two current
  callers rather than one hypothetical adapter.

### 5. Share the sandboxes' free-fly transform kernel

- Files and symbols: `src/sandbox.rs::fly_camera` and
  `src/track_sandbox/mod.rs::fly_camera`; a shared pure kernel would naturally live in
  `src/camera.rs`.
- What is wrong: duplicated knowledge. The systems have distinct camera marker types and run
  conditions, but their yaw/pitch clamp, mouse sensitivity, planar WASD basis, altitude keys,
  normalization, speed, and real-time integration are the same implementation.
- Estimated change: DERIVED about 30–45 source lines deleted. Keep the thin ECS adapters and move
  only the `Transform`/input/delta-time arithmetic.
- Behaviour-preserving confidence: high; the current bodies are textually identical apart from
  comments.
- Applicability / C2: safely applicable; no owner decision. A pure kernel with multiple adapters is
  a visible C2b pattern, so it should be named in that change's report rather than arriving as a
  side-effect.

### 6. Centralize capped FIFO entity eviction

- Files and symbols: `src/debug.rs::spawn_impact_marker`,
  `src/vfx/billboard.rs::spawn_billboard_ring`, `src/vfx/muzzle.rs::spawn_muzzle_light`, and the
  currently excluded `src/vfx/trail.rs::on_fire_shell`.
- What is wrong: duplicated knowledge. Each site pushes an entity to a `VecDeque`, pops from the
  front while over a cap, and `try_despawn`s each evictee. That is one leak-bound rule repeated
  across debug markers, billboards, muzzle lights, and trails.
- Estimated change: DERIVED about 20–35 source lines after every caller can migrate.
- Behaviour-preserving confidence: high if the shared operation remains push-then-evict,
  oldest-first, and continues using `try_despawn`.
- Applicability / C2: not applicable in this run because `src/vfx/trail.rs` is excluded. No owner
  decision under C2. This is a reusable C2b pattern and should be introduced as its own explicit
  change once the active trail work clears.

### 7. Put closed-loop winding and outward-normal math in one track helper

- Files and symbols: `src/track/shadow_proxy.rs::ribbon_mesh` and
  `src/track_sandbox/suspension_viz.rs::{loop_winding,outward_normal}`.
- What is wrong: duplicated knowledge. The game shadow ribbon and the sandbox grip overlay both
  derive loop winding from signed area, then turn a normalized segment tangent into its outward
  normal. A sign drift would make either the shadow tube turn inside-out or the sandbox draw grip
  stations inward.
- Estimated change: DERIVED about 20–35 source lines. The helper must accept both cyclic point lists
  and lists with an explicit repeated closing point without changing either caller's segment walk.
- Behaviour-preserving confidence: medium-high; the formula is the same, but the input closure
  conventions require focused tests before consolidation.
- Applicability / C2: safely applicable; no owner decision. Keep mesh winding correction and gizmo
  drawing in their existing modules.

### 8. Remove duplicate present-mode offering assertions from the probe test

- Files and symbols: `src/settings/probe.rs::fake_capability_lists_gate_the_ladder_correctly` and
  `src/settings.rs::the_offered_rungs_follow_the_probe`.
- What is wrong: redundant tests. The settings test already exhaustively pins which
  `PresentCaps` state offers each `VsyncMode`; the probe test repeats those same failure conditions
  after testing the separate and valuable `wgpu::PresentMode`-list-to-`PresentCaps` distillation.
- Estimated change: DERIVED about 15–25 test lines deleted from `src/settings/probe.rs`.
- Behaviour-preserving confidence: high. Keep every `caps_from_present_modes` assertion in the
  probe test and remove only the repeated `offered` helper/assertions.
- Applicability / C2: safely applicable; no owner decision. This touches an in-module test, not one
  of the protected top-level files under `tests/`.

### 9. Reuse the phase-law `fold` in the sandbox belt tests

- Files and symbols: `src/track/gear_phase.rs::fold` and the test-local
  `src/track_sandbox/belt.rs::tests::fold`.
- What is wrong: duplicated knowledge. The test helper is an exact copy of the production phase
  fold it says it mirrors.
- Estimated change: DERIVED about 5–10 test lines.
- Behaviour-preserving confidence: high.
- Applicability / C2: safely applicable; no owner decision. This is small, so it ranks below
  production duplication and broader redundant-test cleanup.

## Surveyed areas with no backlog-worthy finding

- `src/vfx/impact.rs`, `src/vfx/muzzle.rs`, `src/vfx/billboard.rs`,
  `src/vfx/prewarm.rs`, and `src/vfx/ember.rs`: billboard spawn, per-instance material setup,
  lifetime aging, and long-lived ground-mark isolation already converge on
  `spawn_billboard{,_ring}` and the shared aging system. Apart from the capped-entity operation
  above, the remaining code is effect-specific data and behavior. The excluded trail file was read
  only enough to identify the capped-ring occurrence and was not edited.
- `src/settings.rs`, `src/settings/probe.rs`, `src/settings/limiter.rs`, and
  `src/settings/store.rs`: model normalization, persistence, and frame limiting have clear owners.
  The similar `FrameCap` and `UiScalePercent` slider arithmetic has materially different ladder
  rules, so abstracting it would move clear code rather than delete knowledge. The hand-rolled
  settings interaction already has the dedicated C2 proposal in
  `design/bevy-ui-widgets-migration-proposal.md`; it was not re-filed here, and the excluded
  `src/settings/ui.rs` was not edited.
- `src/track/{sim,view,wrap,route,forces,rig_geom,marker_model,gear_phase,link_view}.rs` and
  `src/track_sandbox/`: the phase law, route construction, travel field, and sim/view split already
  share their domain kernels. Similar force/hash loops encode different byte streams or different
  sim/view facts and are not duplicates. The remaining concrete exceptions are ranked above.
- `src/tank/{model,spawn,servo,view}.rs`, `src/bake.rs`, `src/damage.rs`, and `src/shooting.rs`:
  spawn-time sim construction and named query shapes are already centralized. The pose-verifier
  duplicate and collision-proxy construction are the genuine misses found.
- `src/spec.rs`, `src/track/rig_geom.rs`, and `src/track/marker_model.rs`: RON parsing, semantic
  validation, and model measurement have distinct responsibilities. The repeated finite/range
  checks carry different accepted domains and diagnostics; a generic validator would obscure those
  contracts.
- `src/terrain_grid.rs`, `src/track/oracle.rs`, and `src/world.rs`: allocations found here are
  setup, asset-build, or test paths rather than per-tick churn. The public diagnostic geometry
  functions with no live caller are explicitly documented authoring surfaces, not accidental dead
  code.
- `src/drive_hud.rs`, `src/crew_ui.rs`, `src/hud.rs`, `src/sight/reticle.rs`, and
  `src/net/debug_hud.rs`: network diagnostics are already throttled at their declared interval.
  Other formatted text reflects tick- or render-cadence state; without a measurement, adding caches
  or slower refresh behavior would be state growth or an observable behavior change rather than a
  safe simplification.
- `src/headless_test.rs`, `src/track/transmission/tests.rs`, and the in-source suites surveyed near
  changed modules: their size comes from distinct scenario gates, not repeated failure conditions.
  Only the present-capability overlap above met the deletion bar. The protected top-level `tests/`
  tripwires were inventoried from the quality standard and not edited.
- Non-excluded `src/net/` modules, especially `grip/{authority,checkpoint,client}.rs`,
  `grip_battery.rs`, `harness.rs`, `shot_loss.rs`, and `server.rs`: repeated iteration shapes write
  different exact/coarse hashes, checkpoint schemas, or test evidence and must remain separate.
  Excluded net files were not edited.

## Owner decisions deliberately left alone

No new C2 proposal arose from this survey. The significant behavior-changing library adoption—the
settings controls moving from raw cursor math to Bevy widgets—was already surfaced by the prior
branch commit in `design/bevy-ui-widgets-migration-proposal.md`, so this backlog neither duplicates
that proposal nor applies it. The headless-root and capped-ring entries are C2b pattern changes, not
C2 behavior changes; they are deferred because current exclusions prevent a complete, visible
application.
