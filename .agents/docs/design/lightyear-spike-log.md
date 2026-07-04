# lightyear spike log — increments 5 & 6

Working log, appended as I go. Dated 2026-07-03. See `lightyear-spike-map.md` for the API
reference this implementation follows.

## Setup / API verification (before writing code)

- 2026-07-03: Confirmed `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` holds real
  lightyear 0.28.0 + subcrate sources (matches the map's research). Also found the prior
  session's clone still on disk at
  `/private/tmp/claude-502/.../d4c36e52.../scratchpad/lightyear-research/lightyear-src` — used
  both interchangeably, cross-checked a few spots.
- `LightyearAvianPlugin` (from `lightyear_avian3d`, re-exported as `lightyear::avian3d::plugin::*`)
  confirmed: `AvianReplicationMode::Position` default, disable list is exactly
  `PhysicsTransformPlugin` + `PhysicsInterpolationPlugin` (module doc, `crates/integration/avian/src/plugin.rs`
  — wait, actual path in this checkout is `lightyear_avian3d-0.28.0/src/plugin.rs`, differs from
  map's cited path but same content). `avian_3d_character`'s `shared.rs` ALSO disables
  `IslandPlugin`/`IslandSleepingPlugin` explicitly — matches map §8 exactly, following that.
- `Link::new(Option<RecvLinkConditioner>)` confirmed — `RecvLinkConditioner = LinkConditioner<RecvPayload>`
  type alias in `lightyear_link`. `LinkConditionerConfig::new(incoming_latency, incoming_jitter, incoming_loss)`
  is the receive-side (inbound) conditioner — applies to packets arriving at whichever peer holds
  it. Putting it on the client's `Link` conditions what the client receives from the server
  (server→client latency); this is the map's suggested mechanism for "prediction runs genuinely
  ahead."
- `PredictionMetrics` (`rollbacks: u32`, `rollback_ticks: u32`) is a real resource, populated by
  `PredictionDiagnosticsPlugin`, which `PredictionPlugin::build` mounts unconditionally
  (`crates/replication/prediction/src/plugin.rs:256`: `app.add_plugins((PredictionDiagnosticsPlugin::default(), RollbackPlugin))`).
  `PredictionPlugin` is itself mounted by `ClientPlugins` whenever the `prediction` feature is on
  (default-on). So `Res<PredictionMetrics>` is available on the client with zero extra plugin
  wiring — no need to add `PredictionDiagnosticsPlugin` by hand. Exported at
  `lightyear::prelude::PredictionMetrics`. This is the primary rollback-fired signal; the
  Position-discontinuity `ROLLBACK-SNAP` log is the secondary/backup per the task's success
  criteria.
- `PredictionTarget = ReplicationTarget<Predicted>` (type alias), `PredictionTarget::to_clients(NetworkTarget)`
  — both exported via `lightyear::prelude::*` (feature `prediction`, default-on).
- Avian3d 0.7's `prelude` re-exports `dynamics::prelude::*` which covers `IslandPlugin`/
  `IslandSleepingPlugin`; already how `src/lib.rs`/`sandbox.rs` import avian types
  (`avian3d::prelude::{...}`), so no new import path needed.

- **Discrepancy vs task wording**: avian3d 0.7 has NO `ExternalForce`/`ExternalTorque` components
  (those existed in older avian versions) — force/torque/impulse application in 0.7 goes through the
  `Forces` `QueryData` helper (`avian3d::prelude::forces::Forces`, `.apply_force()`/`.apply_torque()`/
  `.apply_linear_impulse()`), confirmed by grep (zero hits for `ExternalForce`/`ExternalTorque` in
  the avian3d 0.7.0 source tree; `Forces` doc comment explicitly documents this as the mechanism).
  Using `Query<Forces>` for the stub movement system and the server-only perturbation impulse.

## Increment 5 — plan

- Remove `SimPlugin` from `spike_server` per the task; compose physics directly with
  `PhysicsPlugins::default().build().disable::<PhysicsTransformPlugin>().disable::<PhysicsInterpolationPlugin>().disable::<IslandPlugin>().disable::<IslandSleepingPlugin>()`
  + `LightyearAvianPlugin { replication_mode: AvianReplicationMode::Position, ..default() }`.
- Register `Position`/`Rotation`/`LinearVelocity`/`AngularVelocity` in `net.rs` (shared, `net`
  feature only — no game-build impact) with `.replicate().predict()` + rollback conditions +
  correction/interpolation fns exactly per map §5 (verbatim from `avian_3d_character`'s
  `protocol.rs`).
- Cargo: add `avian3d` feature to the `lightyear` dep (meta-crate re-export) — map §1 confirms this
  is what makes `lightyear::avian3d::plugin::*` resolve. Add `lightyear_avian3d` directly too per
  the map's Cargo.toml proposal (explicit `3d`/`f32` features) since the meta-crate's `avian3d`
  feature alone doesn't add the plugin, just the re-export path — cleanest to depend on both as
  the map recommends.
- **Correction**: `lightyear`'s `avian3d` feature = `["dep:lightyear_avian3d", "lightyear_replication?/avian3d"]`
  (checked `crates/core/lightyear/Cargo.toml:155`) — it already pulls `lightyear_avian3d` as an
  optional dep of the meta-crate itself. A *separate* `lightyear_avian3d` direct dependency is
  unnecessary; enabling the meta-crate's `avian3d` feature alone is sufficient (confirmed via
  `cargo add --dry-run`). Did not add a second direct dependency.
- **Correction**: avian3d 0.7 has no `ExternalForce`/`ExternalTorque` components (confirmed zero
  grep hits) — force/torque/impulse application goes through `avian3d::prelude::Forces`, a mutable
  `QueryData` (`.apply_force()`/`.apply_torque()`/`.apply_linear_impulse()`), gated behind two
  traits (`ReadRigidBodyForces`, `WriteRigidBodyForces`) that must be imported for the methods to
  resolve (not auto-imported by `avian3d::prelude::*` — an easy first-compile trap, hit and fixed).
- Cargo: `avian3d`'s `Position`/`Rotation`/`LinearVelocity`/`AngularVelocity` need
  `avian3d/serialize` (adds `Serialize`/`DeserializeOwned` impls) for lightyear's
  `.replicate()` to accept them — gated the feature behind `net` in `[features] net = ["dep:lightyear", "avian3d/serialize"]`
  rather than turning it on unconditionally, so the plain default build's avian3d feature set is
  provably untouched (constraint: "default build must remain behaviorally untouched").

## Increment 5/6 CSP defect diagnosis + fix — 2026-07-03

Two independent bugs, both in code added this session (`src/net.rs`, `src/bin/spike_server.rs`),
neither upstream. Diffed line-by-line against `avian_3d_character`'s `shared.rs`/`protocol.rs`/
`client.rs`/`server.rs`/`main.rs` (scratchpad clone) — plugin set, disable list, schedule
placement, and correction/interpolation wiring all already matched the reference exactly; the
divergence was narrower than "missing plugin/config."

### Bug 1 — rollback storm (symptom 1: ~632 rollbacks / 1.8 s at zero latency)

`net.rs::plugin` registered `LinearVelocity`/`AngularVelocity` with `.replicate().predict()` and
**no** `.with_rollback_condition(...)`. Per `lightyear_prediction::registry` (confirmed by source:
`crates/replication/prediction/src/registry.rs:112`, doc comment + `SyncComponent: ... + PartialEq`
bound), the rollback comparator defaults to `PartialEq::ne` — exact bit equality — when no
condition is supplied. `avian_3d_character`'s `protocol.rs` gives BOTH velocity components an
explicit `>= 0.01` length-threshold condition (same shape as Position/Rotation); ours only did that
for Position/Rotation. Any one predicted component voting "rollback" forces the whole entity to
roll back (`PredictionRegistry::should_rollback_check`, called per-component, OR'd), so f32 solver
noise in LinearVelocity/AngularVelocity (never bit-exact between client and server, even in
straight-line steady state) tripped a rollback on nearly every packet.

**Fix**: added the same `(a.0 - b.0).length() >= 0.01` condition to both, verbatim shape from the
reference. Result: 632 -> 6 rollbacks in the first 1.8s, and those 6 are legitimate (5 during
initial spawn/gravity-settle in the first ~0.2s, 1 exactly coincident with the server's forced
perturbation at t=2s). Zero rollbacks in the steady-state cruise between those two events.

### Bug 2 — "oscillation" (symptom 2: 1000+ ROLLBACK-SNAP hits) was a detector false positive, not CSP oscillation

Root cause was NOT rollback/correction fighting itself — `PredictionMetrics.rollbacks` stayed flat
(6 total) across the whole run even while `ROLLBACK-SNAP` (the >0.5 m one-tick position-delta
detector) fired ~1000 times. The server's one-shot perturbation impulse
(`spike_server.rs::perturb_after_delay`, `IMPULSE: f32 = 4_000_000.0` N*s on the 57,000 kg tank)
injects `4e6 / 57e3 ≈ 70 m/s` of *instantaneous* lateral velocity — far above the tank's own
~4-15 m/s cruise speed under `DRIVE_FORCE`. At 70 m/s, a single **legitimate** FixedUpdate tick
moves `70 * (1/64) ≈ 1.09 m` — already past the detector's 0.5 m bar with zero misprediction
involved; the "oscillation" was the tank coasting fast on a curved (throttle-forward +
impulse-lateral) path while decaying under friction, sampled once per render `Update` frame by
`log_snap`. Confirmed by cross-checking server and client `position=` logs: both sides show the
*same* large-radius curved trajectory (X growing to 400+, Z to -150+ within ~9 s), converging to
the same rest point — i.e., real, agreeing physics, not divergence. The task brief's own "+1.09 m"
measurement is explained exactly by this (70 m/s * one tick).

**Fix**: reduced the perturbation impulse from `4_000_000.0` to `171_000.0` N*s (~3 m/s lateral
delta-v on the 57 t body) — still an order of magnitude above the 0.01 rollback threshold
(guarantees exactly one misprediction/rollback) but small next to cruise speed, so no single tick's
legitimate motion approaches the snap detector's 0.5 m bar. Comment left in `spike_server.rs`
explaining the sizing.

### Before/after (SPIKE_LATENCY_MS=0, pure loopback, ~12 s run)

| | rollbacks | ROLLBACK-SNAP |
|---|---|---|
| baseline (both bugs) | 632 | 1003 |
| bug 1 fixed only | 6 | 1002 |
| both fixed | 6 | 1 (initial spawn/gravity settle) |

With the default conditioner (100 ms/20 ms): 13 rollbacks, 9 snaps over the same script — more
than zero-latency (expected: prediction genuinely runs ahead, more resimulation windows), but same
qualitative shape as the fixed zero-latency run — sparse, non-oscillating, monotonic convergence to
the same rest position on both sides. No back-and-forth signature in either run's position log.

### What increment 6 (glb rig under prediction) must inherit

- **Every predicted component needs an explicit `.with_rollback_condition(...)` if its natural
  equality is float/bit-exact** — `PartialEq::ne` as a default is a footgun for anything numeric.
  Audit this the moment a new predicted component is added (e.g. turret/barrel joint angles if
  those become predicted state) — don't rely on "it compiled" as a signal.
- **The >0.5 m ROLLBACK-SNAP heuristic is speed-relative, not an absolute rollback signal.** At tank
  cruise speeds (~13-16 m/s) a single ordinary tick is already ~0.2-0.25 m; anything that adds
  velocity (ramming, knockback, a shell impact) can trip it without any misprediction. Cross-check
  against `PredictionMetrics.rollbacks` before calling something a CSP bug — the metric is the
  authoritative signal, the snap log is a distance-based heuristic that reads speed as if it were
  divergence.
- **Verified NOT the cause, don't re-litigate**: plugin set, disable list
  (`PhysicsTransformPlugin`/`PhysicsInterpolationPlugin`/`IslandPlugin`/`IslandSleepingPlugin`),
  `LightyearAvianPlugin` config, `add_linear_correction_fn()`/`add_linear_interpolation()` wiring,
  schedule placement of the shared movement system (`FixedUpdate` on both sides, matches reference)
  — all already correct, byte-for-byte equivalent to `avian_3d_character`. `FrameInterpolationPlugin`
  is reference-side render-only (mounted in their `renderer.rs`, gui feature) and legitimately absent
  from our headless spike; its correction/interpolation queries (`&mut FrameInterpolate<C>`) simply
  don't match without it, which is inert, not broken — confirmed by reading
  `crates/replication/prediction/src/correction.rs`.
- No upstream lightyear 0.28 issue implicated — searched for existing GitHub issues about the
  `PartialEq::ne` rollback default, found none; it's a documented (doc-comment) default, not a bug,
  and the reference example itself works around it explicitly for the same two components we missed.

## Increment 6 verdicts — 2026-07-03 (verified live, 0 ms and 100 ms runs)

1. **Binder fires exactly once** per tank despite rollback replays (`rig_binds=1` every run) —
   the map §8 reasoning holds: `on_tank_ready` observes scene-ready (outside `FixedMain`), and
   rollback only re-runs `FixedMain`.
2. **Child colliders track through rollback**: `TURRET-DRIFT=0` in all runs — the turret collider
   holds its relative pose through the forced-perturbation rollback
   (`update_child_collider_position` works on our glb-built rig). No panics, no NaN.
3. **Spawn-before-bind is fatal if naive, solved by Static-until-bind**: a Dynamic root with no
   collider free-falls through the ground for the whole async glb load (measured y = −425).
   Pattern: spawn `RigidBody::Static`, flip to Dynamic on `Added<Rig>` (`activate_bound_rigs`,
   shared both ends). This is the answer to the map §8 UNCERTAIN — `PredictionTarget`-at-spawn
   itself is fine; it's the *body activation* that must wait for bind.
4. **Client root needs an explicit `Transform`**: a replicon-spawned root has only replicated
   components; without `Transform` the scene hierarchy under it never gets `GlobalTransform`s
   (Bevy B0004) and the binder captures a wrong rig. Added to `spike_tank_rig` with a why-comment.
5. **Single-entity model confirmed empirically** (STRUCT dump): one entity carries
   remote+predicted+rig+body. Earlier "two stacked tanks / +1.7 m divergence" was a diagnostic
   artifact — `With<Predicted>` position logs were catching the tank's own *child collider
   entities* (turret at +1.44 m rest offset, correctly). Logs now scoped `With<SpikeTank>`.
6. **Convergence**: server vs client rest position identical to ~7 significant figures at both
   0 ms and 100 ms+jitter (e.g. (7.21099, −0.28296, −32.93752) both ends).
7. **OPEN for next session — rollback rate**: `PredictionMetrics.rollbacks` ≈ 430 over ~15 s at
   100 ms (vs 13 for the increment-5 primitive). Invisible (snaps=2, converges) but it's ~30
   full re-simulations/s of CPU. Suspect: contact-rich rig (many child colliders) makes solver
   noise exceed the 1 cm thresholds far more often than a single-box body. Candidates: loosen
   per-component thresholds; check whether velocities need a coarser condition on multi-contact
   bodies; confirm child-collider Position/Rotation aren't accidentally in the predicted set.

- 2026-07-03: New doc `lightyear-step7-map.md` answers the above (child colliders are confirmed
  NOT in the predicted set — fix is threshold/input-delay tuning) plus predicted-shell spawning
  (`PreSpawned`, do not gate on `is_in_rollback`), `local_rollback::<C>()` bounds and its
  child-entity limitation (blocks `ServoState`/`Reload`/`Suspension` as currently structured —
  architecture-determining), `is_in_rollback` usage, `InputDelayConfig::fixed_input_delay`, and
  pause-under-lightyear. Read before starting step 7.

## Step 7 — SimPlugin wired into the spike bins (2026-07-03)

- 2026-07-03: Grounding pass done (map §1/§3/§4/§5/§7 + this log's increment 5/6 sections + all
  step-7 code targets). Confirmed before writing code: no sim system in `driving`/`shooting`/
  `aim::sim_plugin`/`firecontrol::sim_plugin`/`ballistics` gates on `Controlled` — grep hits for
  `With<Controlled>` are exclusively client-side (aim.rs HUD indicators, tank.rs Tab swap,
  damage.rs `ControlledTank` HUD SystemParam). The Milestone-A claim holds; the bins need no
  `Controlled` anywhere.
- **Derives (game modules, additive-only)**: `Clone + PartialEq + Debug` added to `ServoState`
  (tank.rs), `Reload` (shooting.rs), `Suspension` + `DriveState` (driving.rs); `DriveState` also
  `struct` → `pub struct` (it was module-private; `local_rollback::<C>()` needs the type nameable
  from net.rs). `Suspension`'s `Option<Vec3>`/`f32` fields derive fine as predicted. New
  feature-gated re-exports in lib.rs: `DriveState`, `Suspension`, `Reload`, `ServoState`,
  `ConsumeCommandEdges`, `AppState` (bins must open the `GameplaySet` gate themselves).
- **Rig decoration (map §7 design, amendment 1)**: `net.rs::decorate_rig_children` — a plain
  `Added<Rig> + With<Predicted>` query system (Update), inserts `DeterministicPredicted::default()`
  (skip_despawn: false) on `rig.turret`/`rig.gun`/`rig.muzzle` + every `Roadwheel` descendant.
  Registered `local_rollback::<DriveState/ServoState/Reload/Suspension>()` in `net::plugin` —
  safe on the server too (silently no-ops without `PredictionPlugin`, map §3.2). Root needs
  nothing extra (`Predicted` already carries `DriveState` history via `local_rollback`).
- **Rollback thresholds coarsened** (map §1c recommendation): named consts in net.rs —
  `ROLLBACK_POSITION_M = 0.05`, `ROLLBACK_ROTATION_RAD = 0.05`, `ROLLBACK_VELOCITY = 0.05`
  (the 1 cm reference values are a capsule character's; correction smoothing hides ≤5 cm on a
  57 t tank). A/B evidence to follow below.
- **PredictionDiagnosticsPlugin**: NOT re-mounted — `PredictionPlugin::build` already mounts it
  unconditionally (`prediction/src/plugin.rs:256`, re-verified in the vendored source); a second
  `add_plugins` would panic on Bevy's duplicate-plugin check. Instead
  `net::log_prediction_diagnostics` reads `DiagnosticsStore` (ROLLBACKS/ROLLBACK_DEPTH paths)
  every ~5 s in the client bin.
- **Input delay lever**: `SPIKE_INPUT_DELAY_TICKS` (default 0) → when >0, inserts
  `InputTimelineConfig::default().with_input_delay(InputDelayConfig::fixed_input_delay(n))` on
  the Client entity before Connect (map §5.3 attach point). Off by default — pure A/B lever.
- **TERRAIN DECISION — spike_ground dropped, world::plugin's terrain wins.** Two grounds would
  otherwise coexist (SimPlugin→world::plugin spawns the game terrain at Startup on both sides).
  Investigated the real blocker candidates first: world.rs positions terrain via `Transform`
  (+ scale-derived collider sizing) while `physics_plugins()` disables avian's
  `PhysicsTransformPlugin` — BUT `LightyearAvianPlugin` (Position mode, `update_syncs_manually:
  false` default) mounts its own `transform_to_position` + transform propagation in
  `RunFixedMainLoop` (vendored `lightyear_avian3d-0.28.0/src/plugin.rs:163`), and collider
  scale-from-Transform lives in `ColliderBackendPlugin` (`avian3d-0.7.0/src/collision/collider/
  backend.rs:472`), which is NOT in the disable list. So the game terrain gets correct
  Position/scale under the netcode composition, and it's already `Layer::Terrain`-tagged — which
  the old `spike_ground` never was, meaning the wheels' suspension rays (filtered to Terrain,
  tank.rs `RayCaster::with_query_filter`) would have sailed straight through it. `spike_ground`
  removed from net.rs + both bins; live-run evidence (wheels grounded) to follow.
- **Input bridge**: `net.rs::bridge_action_state_to_tank_command` (FixedUpdate,
  `.before(ConsumeCommandEdges)`): whole-struct copy of `ActionState<TankCommand>.0` into the
  entity's `TankCommand` for entities carrying both. Ordering reasoning: `consume_edges` clears
  `fire_primary`/`crew_swap` at tick end; the bridge must land this tick's edges first — the
  identical constraint `shooting::fire` already declares. NOT gated on `is_in_rollback`: replay
  restores `ActionState` per-tick from the InputBuffer (lightyear's own systems), so re-copying
  during replay feeds the *historical* input — gating would leave `TankCommand` stale through
  replay (map §3.4's "no gate needed" class). `attach_command` (`On<Add, Tank>`) supplies the
  `TankCommand` side on both ends: the rig bundle includes `Tank`, server spawn + client
  `attach_predicted_rig` both trigger it.
- **Stub retired**: `drive_stub_movement` + `DRIVE_FORCE`/`STEER_TORQUE` consts deleted from
  net.rs; `spike_tank_physics()` never existed under that name — the increment-5 leftovers were
  the stub system + `spike_ground()`, both now gone. `fire` stays ungated by `is_in_rollback`
  (coordinator decision: replay may duplicate a local tracer — cosmetic; PreSpawned is a later
  increment, deliberately NOT added anywhere this step).

### Step 7 verification runs (2026-07-03, logs in the session scratchpad `step7_*.log`)

- **Gates**: `cargo fmt --check` clean; `cargo clippy --all-targets` AND `--features net` both
  zero warnings; `cargo test` 14/14 (incl. `headless_test::sim_boots_and_drives_headless` —
  headless_test now mounts `PhysicsPlugins` + `sp_spawn_plugin` alongside `SimPlugin` itself,
  matching the composition-root split).
- **LIVE BUG 1 — bridge ordering (fixed)**: first run's fire click was silently lost. With the
  bridge only `.before(ConsumeCommandEdges)` it is UNORDERED vs `fire`; measured: `fire` ran
  first (read the stale command), then `consume_edges` cleared the edge the bridge had just
  written — no tick ever consumed the click (server had `fire_primary=true` in `ActionState`,
  reload never left 0.0). Fix: bridge is `.before(GameplaySet)` (every consumer AND the
  edge-clearer live in that set). `GameplaySet` re-exported (net-gated) for this;
  `ConsumeCommandEdges` re-export reverted (unused).
- **LIVE BUG 2 — map §7 amendment 1 EMPIRICALLY FALSIFIED (fixed)**: with
  `DeterministicPredicted::default()` (skip_despawn: false) all 19 decorated children were
  DESPAWNED ~16 ms after decoration ("Entity despawned ... is invalid" warnings), rig broken
  client-side, 201 rollbacks/15 s of permanent desync. Cause: `deterministic_despawn` drains on
  every rollback and despawns entities whose registration tick > rollback_tick — and rollbacks
  fire continuously through the post-bind suspension-settle burst, so "bind seconds after spawn"
  does NOT put decoration clear of live rollback targets: the map's "vanishingly narrow window"
  is the common case. Switched to `skip_despawn: true` (grace: `DisableRollback` for
  `enable_rollback_after` = 20 ticks, then full participation) — the very variant the
  `deterministic_replication` example's decorating observer uses. Result: zero despawns, all
  evidence clean. Map §7's final verdict amendment 1 should be treated as reversed.
- **A/B rollback rate** (client `--simulate-input`, ~18 s runs, thresholds 0.05):
  - `SPIKE_LATENCY_MS=0`: **22 rollbacks** total (all in the ~1.3 s post-bind settle burst,
    zero after), 1 snap (settle), depth ~3.
  - `SPIKE_LATENCY_MS=100` (+20 jitter): **152 rollbacks** vs increment-6's 430/15 s with the
    stub at 1 cm thresholds — ~65% drop with the FULL sim now running (16-wheel suspension +
    brush friction + servos, a much noisier body than the stub). Distribution: ~10/s settle,
    ~28/s only during full-throttle+steer maneuvering, ~8/s coasting, ~0 at rest. 1 snap.
  - `SPIKE_INPUT_DELAY_TICKS=2` @100 ms: 158 — lever wired+functional but no measurable effect
    here; the residual rate is threshold-tripping solver noise under maneuver, not
    prediction-window depth. Keep at 0.
- **Real-sim evidence (every criterion, client log lines)**:
  - Suspension grounded: `SIM-EVIDENCE wheels_grounded=16/16` (14/16 momentarily during settle).
  - Hull ramped, not instant: position curve 0 → (0.9,−2.3) → (1.3,−11.0) → (2.2,−20.3) over
    2 s samples under throttle 1.0 + steer 0.3 (INPUT_RAMP visibly easing in).
  - Turret slewed to the scripted aim: `turret_angle=Some(-0.2449951)` — exactly
    atan2(−200, 800) = −0.24498 rad for the hull-local aim (200, 0, −800). (Angle converges
    within the first 2 s sample; the "before" state is the servo's 0.0 rest.)
  - Reload cycled: `reloads=[0.0, 0.0, 1.984375]` one sample after the tick-300 fire (3.0 s
    MainGun reload minus ~1 s), back to 0.0 two samples later. NOTE: three `Reload`s exist
    (MainGun + Coax + hull MG) — an earlier `.iter().next()` evidence read sampled the wrong
    weapon and masked the working fire; log now prints all.
  - Rig binds once every run (`rig_binds=1`), 19 children decorated (turret+gun+muzzle+16
    wheels), convergence: client/server positions on the same trajectory within sample-time
    offset (e.g. client (2.29,0.018,−19.94)@40.10 vs server (2.25,0.018,−19.53)@39.88).
- **Forced-rollback + fire pass** (`SPIKE_FIRE_TICK=110`, 100 ms): fire landed inside the
  perturbation rollback burst (perturbation @47.73, rollbacks 46.9–47.4+, SHELL-SPAWN @48.00).
  `SHELL-SPAWN ... (total=1)` — the tracer did NOT duplicate in this pass; the accepted wart
  did not manifest (recorded as rare, not disproven — PreSpawned remains the later fix). Zero
  panics, zero NaN, zero despawn warnings in all four runs.
- **Terrain**: `world::plugin` game terrain confirmed working headless under the netcode physics
  composition on both ends (wheels grounded 16/16 proves the Terrain-layer rays hit it);
  `spike_ground()` deleted. Tank drives the real test-course world now.

## Step 8 — playable client + prediction toggle (2026-07-03)

- **Design shipped**: windowed `spike_client` mounts `NetClientPlugin` (new in lib.rs: `ClientPlugin`
  minus `state::client_plugin` and `tank::client_plugin`); possession = `claim_input_slot` adds the
  game's `Controlled` alongside the input slot; input = `feed_action_state` (FixedPreUpdate,
  `InputSystems::WriteClientInputs`, `not(is_in_rollback)`) copies the `Controlled` tank's
  `TankCommand` — filled by the game's own writers at render rate — into `ActionState`, closing the
  loop with step 7's reverse bridge. Locally an identity round trip; the buffer is the wire format
  and the rollback-replay source. Esc = spike-local cursor-release menu overlay, `AppState` stays
  `Playing` (no online pause — a paused predicting client desyncs; `state::client_plugin`'s
  pause_physics is exactly what must NOT run under netcode). Toggle = server-side `SPIKE_PREDICT=0`
  swaps the owner's `PredictionTarget` for `InterpolationTarget` at spawn (map §7 option 1);
  `SPIKE_PERTURB=0` drops the forced-rollback impulse for feel runs.
- **Servo replication pulled forward from step 9**: replicated `ServoAngles { turret, gun }` on the
  root, published by the authority from `ServoState` (`publish_servo_angles`, FixedPostUpdate,
  `set_if_neq` so rest is replication-silent), consumed on non-predicted client tanks by
  `apply_servo_angles` writing `ServoCommand.target` — the local servo mechanism chases the
  authoritative angle under its real slew profile, which smooths replication-rate steps for free
  (no interpolation registration, no `interpolate_servos` transform fights). Hull-MG servos
  deliberately not covered (per-weapon laying is its own slice).
- **LIVE BUG 3 — `Interpolated`/`Predicted` markers CANNOT discriminate authority vs replica.**
  First cut keyed "locally simulated" off `Without<Interpolated>`: both bins froze (wheels 0/16,
  positions pinned at spawn). Cause, verified in vendored source (`lightyear_replication-0.28.0/
  src/send.rs:1111,1119`): `PredictionTarget`/`InterpolationTarget` are
  `ReplicationTarget<Predicted>`/`<Interpolated>` and `register_required_components` puts the
  *marker itself on the server entity* — the A-mode server tank carries BOTH `Predicted` AND
  `Interpolated`; the markers are then target-filtered replicated components. The honest
  discriminator is replicon's `Remote` ("arrived by replication"): locally-simulated ⇔
  `Or<(With<Predicted>, Without<Remote>)>` (applied to `activate_bound_rigs` + the ActionState→
  TankCommand bridge); authority ⇔ `Without<Remote>` (publish); non-predicted replica ⇔
  `(With<Remote>, Without<Predicted>)` (apply, rig-attach, B-mode field clears).
- **Non-predicted own tank (B mode) semantics**: full rig binds (camera/HUD/servo-apply need the
  node map) but body stays `Static` — replication owns the pose; `feed_action_state` clears
  `aim`/fire edges/`crew_swap` after the copy (local sim must not slew/fire/swap at zero latency)
  while throttle/steer/range survive multi-tick frames. Known accepted warts, B mode only: no
  local tracer on fire (shell replication still deferred), local HUD reload/damage don't reflect
  server state, wheels don't animate (no local suspension), mouse still orbits while the menu is
  open (GameplaySet no longer gates presentation off — SP got this for free from the pause state).
- **Verification (headless, both modes, SPIKE_LATENCY_MS=100)**: A-mode regression — step-7
  evidence intact: Dynamic on bind, wheels 16/16, turret −0.2449951 exact, reload cycles,
  SHELL-SPAWN total=1, 148 rollbacks/run (step 7: 152), convergence on the same trajectory; the two
  B0004 warnings at bind are pre-existing (reproduced on stashed step-7 baseline, count identical).
  B-mode — server receives 256 throttle commands, drives wheels 16/16 + fires (reload cycles
  server-side); client: 0 rollbacks, 0 local shells, no Dynamic (stayed Static), position tracks
  the server run, `turret_angle` −0.238→−0.2449951 — the replicated-angle chase converges to the
  authoritative lay. Windowed smoke (~25 s, real window): connect → possession → bind → Dynamic,
  zero warnings, and `turret_angle` live-moving (1.71→1.77 rad) = the real camera-ray `commit_aim`
  drove the wire path with no human input.
- **Feel-test recipe (the actual step-8 deliverable, USER runs it)**: terminal 1
  `cargo run --bin spike_server --features net` (add `SPIKE_PREDICT=0` for the B side,
  `SPIKE_PERTURB=0` always for feel runs); terminal 2 `SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10
  cargo run --bin spike_client --features net` (0 for pure localhost). W/S/A/D drive, mouse aims,
  LMB fires, scroll zooms, Esc menu. Ladder: 0 ms → 80 ms → cloud region (Edgegap, first outing).

### Step 8 rollback-storm investigation (2026-07-04, user's run-1 report)

User reported "thousands of rollbacks, jitter when driving fast, sluggish turret, sometimes spam
at standstill" in the windowed client. Bisection (all live runs, scratchpad `soak_*`/`simw_*`/
`sweep*_*` logs):

- **Standstill soak, windowed, 0 ms, 60 s hands-off**: 26 rollbacks, ALL in the 2 s post-bind
  settle burst, then a full clean minute — matches the healthy headless baseline. (First read of
  this run was wrong by one minute of timestamp arithmetic; corrected against log start times.)
- **New diagnostic levers** (kept, spike_client): `SPIKE_SIM_WINDOWED=1` = scripted input in a
  real window (vsync pacing + full presentation, deterministic workload; NetClientPlugin's device
  writers are dead-ended by the reverse bridge, so the script rules). `SPIKE_SIM_AIM_SWEEP=1` =
  aim orbits the tank at ~1.3 rad/s (a player scanning) instead of the constant point.
- **Windowed+script 0 ms: 29. Headless+sweep 0 ms: 19. Windowed+sweep+drive 0 ms: 28.** Frame
  pacing, rendering, and aim churn are ALL exonerated at 0 ms — no storm reproducible.
- **Windowed+sweep+drive at the DEFAULT 100 ms conditioner: 138/19 s** (~10–28/s under maneuver)
  — the step-7 measured rate. "Thousands over minutes of play" = the client was almost certainly
  launched without `SPIKE_LATENCY_MS=0`, so the run-1 report was really a 100 ms+20 jitter run.
  The conditioner default biting a human tester is a footgun — feel-test commands must pin it.
- **Real bug 1 (crash, fixed)**: `update_bore_indicator` panicked (`Dir3::new_unchecked` NaN) —
  for a frame around a networked bind-under-rollback the muzzle's GlobalTransform is zeroed and
  `forward()`'s unchecked normalize dies. Reproduced twice at 100 ms (exit 101/137 at bind).
  Fix: fallible `Dir3::new`, skip the frame. Would have crashed the user's run 2 at spawn.
- **Real bug 2 (sim poisoning, fixed)**: `commit_aim` computes the hull-local aim via
  `hull.affine().inverse()` — a zeroed hull GlobalTransform yields a NaN aim that rides the
  command to the SERVER and NaNs the authority's servo→physics state (any buggy or hostile client
  could do this). `drive_aim_servos` now holds on non-finite aim and on a non-invertible hull
  affine — the command boundary is a trust boundary.
- Post-fix: windowed+sweep 100 ms runs clean (exit 0, zero NaN, turret tracking the sweep, reload
  cycling); headless 0 ms regression 20 rollbacks (baseline 22); all gates green.
- **OPEN for the feel test**: the ~10–28/s maneuver-time rollback rate at 100 ms is measured-
  invisible in state terms (≤5 cm corrections) but each rollback replays ~7–13 physics ticks in
  one frame — the CPU spike, not the correction, may be what reads as "jitter at speed." If run 2
  (80 ms) still feels rough, next levers: `SPIKE_INPUT_DELAY_TICKS=1..2`, threshold re-tune, or
  release-build the client.
