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
