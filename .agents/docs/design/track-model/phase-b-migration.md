# Phase B migration — one locomotion sim, no forks

Status: PLAN v2 (2026-07-17; v2 amends the hold disposition — see §3a). Owner mandate: full
cutover to the track model — "no old code lingering around, conditionals or forks — we're
upgrading to the full end target." The
previously-planned dev-only A/B switch is CANCELLED; parity evidence comes from the
bit-repeatable sandbox harness, the rewritten contract tests, and playtest. The only fork
retained anywhere: the sandbox's two VIEW models (wrap vs chain, `V`).

Inputs: three research sweeps (sandbox sim inventory, deletion blast radius, netcode patterns)

- codex design review (`scratchpad/codex_phaseb_review.md`). Companion to `architecture.md`
  (v3) and the step log (`HQ.md`).

## 0. What research changed about the plan

- **The steering gap does not exist.** Model-4 already has per-column lateral grip with a
  friction ellipse (`LATERAL_GRIP_RATIO 0.55`), per-side belt speeds from `throttle ± steer`,
  and yaw emerging from differential traction at ±half-tread lever arms on a free 6-DOF hull.
  What it lacks: **hill-hold** (no static anchor — a parked tank creeps on slopes: ~16 cm/s
  on 20°, bounded by the friction ellipse, not runaway — ACCEPTED, see §3a) and any lateral
  reaction into belt dynamics (accepted, matches real skid-steer fidelity here).
- **`BlockField` must move into `SimPlugin`** — today it is built only by the client-side view
  plugin; the dedicated server has none. Shared resource, built from `TerrainMap` on revision
  change, consumed by sim forces AND the view (dedupe).
- **Naming**: "belt" on the wire already means MG ammo (`NetBelts`). The track sim state is
  **`TrackDrive`**, not BeltState.

## 1. End state (source of truth map)

```
src/track.rs              facade: view_plugin (exists), sim_plugin (new), terrain_plugin (new)
src/track/oracle.rs       TerrainOracle + BlockField (exists, unchanged)
src/track/route.rs        route core (exists, unchanged)
src/track/chain.rs        chain view solver (exists, unchanged)
src/track/wheels.rs       view wheel lift (exists, unchanged)
src/track/view.rs         chain/link/wheel view (exists; belt source → TrackDrive)
src/track/terrain.rs      NEW: TrackField resource built from TerrainMap (SimPlugin-mounted,
                          shared by server + clients; view.rs's private copy dies)
src/track/forces.rs       NEW: pure force math — support columns, traction ellipse,
                          engine/governor belt dynamics. No ECS. Consumed by sim.rs AND the
                          sandbox (source of truth). NOTHING beyond the sandbox law (§3a).
src/track/sim.rs          NEW: ECS adapter — TrackDrive component, force system in
                          SimPhase::DrivingForces, capability gate, tick-truth pose reads.
src/driving/              DELETED (suspension.rs, traction.rs, contact.rs, susp_trace.rs).
src/track_sandbox/        model4 physics becomes a thin adapter over track::forces; the two
                          VIEWS (wrap V chain) stay. Models 1–3 (lab archaeology) DELETED.
```

`TankSpec`: `drivetrain:` + `suspension:` blocks die; `track:` grows `powertrain:` +
`support:` sections (ADR-0011 — all authored, no defaults). Surface friction (μ, lateral
ratio, slip saturation) stays a sim CONST — ADR-0007 bucket 3, a property of the
track–ground pair destined for the terrain mechanic, deliberately not vehicle spec. `Roadwheel` loses
`#[require(Suspension)]`; `TankSim.anchors` dies outright (no hold state exists — §3a).

## 2. TrackDrive — the sim state contract

```rust
/// Per-tank tracked-drivetrain sim state. Owner-predicted, replicated to remotes, rolled
/// back — the LinearVelocity registration pattern (NOT NetCrew snap-to, NOT local_rollback).
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackDrive {
    pub throttle: f32,                // shaped command (INPUT_RAMP slew of TankCommand)
    pub steer: f32,
    pub sides: [TrackDriveSide; 2],   // [left, right]
}
pub struct TrackDriveSide {
    pub speed: f32,     // belt surface speed (m/s) — engine/governor/reaction integrated
    pub phase: f64,     // unbounded belt travel (m) — advects force stations AND drives the
                        // view (view wraps by belt_len at presentation)
}
```

- Registration: `.replicate().predict().with_rollback_condition(..)` with float thresholds
  (throttle/steer/speed/phase deltas) + `note_if_tripped` attribution; `WIRE_SURFACE` entry;
  `WIRE_SURFACE_HASH`/`WIRE_TYPES_HASH` re-pinned; `PROTOCOL_REV` bump.
- Joins the determinism hash: new `hblt` stream in `trace::hash_tank_state` (speed +
  phase-bits per side); `anc`/`drv` streams retire with their state.
- `DriveState` dies. Input shaping (`DRIVE_RAMP` smoothing of TankCommand) moves into the
  belt-dynamics step — the governor chasing `command × max_speed` IS the smoothing; one model
  instead of two.
- Phase is sim state (stations advect with it → it affects forces): rolls back, replicates.
  Bonus: remote views get exact link phase — no client integration drift.

## 3. The force model (per side, per fixed tick — all in track::forces)

1. Stations: rest pin-line loop from the spec gear (same `build_route` circles as the view's
   feasibility gate), resampled at pitch, advected by `phase`. Rest circles — road-wheel
   articulation stays view-only (research: it carried zero force in the sandbox too).
2. Per station × 3 lateral columns (Simpson weights from shoe width): directional field depth
   on the outer face (pin/mid/pin), clipped-linear pressure profile → area/centroid/length.
3. Support: penalty spring along belt inward normal − normal-velocity damping, engagement
   ramp, `apply_force_at_point` at the centroid (roll/pitch/weight transfer lever-arm
   implicit).
4. Traction: slip vs `TrackDrive.speed` along drive direction; lateral scrub; friction
   ellipse (`μ·load`, lateral ratio); longitudinal force accumulates into belt reaction.
5. ~~Hold~~ REMOVED (v2, §3a): no anchor pass — the model is the clean sandbox slip law.
6. Belt dynamics: constant-power engine curve (`P/v` capped at F_max), governor toward
   `command × max_speed`, reaction subtraction, reflected inertia, clamp; phase advection.
7. Capability gate: `Drive` capability zeroes COMMAND (not the contact model) — a dead
   engine still grips; it just cannot thrust.

Sim discipline: pose from tick-truth `Position`/`Rotation` (never GlobalTransform — model4's
one game-illegal habit), velocity via Avian `Forces::velocity_at_point`, terrain via
`TrackField` (pure analytic — no SpatialQuery in the drive path, no BVH rollback dependency).
No `Replaying` gate (sim state must replay). Runs in `SimPhase::DrivingForces` + `GameplaySet`
(preserves the "drive samples velocity before fire impulse" cross-phase contract).

## 3a. Hold disposition (v2 — owner correction, 2026-07-17)

The v1 plan transplanted the retired raycast sim's LuGre/Karnopp hold (brush anchor,
STICK_* constants) into the new core, plus powertrain numbers reverse-engineered from the
old sim's feel (soft governor ≈ the old rolling-resistance slope). Owner verdict: the old
driving model was a pure placeholder — nothing structural carries over. Disposition:

- **The force law is the sandbox law, exactly** — commit-B bit-parity was re-proven after
  the hold removal (reversal harness, `cmp` clean).
- **Tiger numbers are sandbox-SCALED, not old-feel-derived**: stiff sandbox governor
  (60 kN·s/m), inertia and support scaled by the 57 t / 26.5 t mass ratio (same ~5 cm
  static sink), force cap 100 kN/side (keeps 20° climb / 30° stall from physics alone).
- **Slope-parking creep is an ACCEPTED known gap** (~16 cm/s on 20°, bounded equilibrium).
  If playtest wants hill-hold it arrives as the per-element bristle extension of THIS
  model (static friction inside the slip law, codex phase-B review) — never the old
  brush anchor.
- `DriveState`-era input shaping survives ONLY as `INPUT_RAMP` command slew in sim.rs
  (vehicle input feel, deliberately separate from the belt governor — codex #9).

## 4. Spec mapping (feel continuity, Tiger)

| Old (dies) | New (track: section) | Tiger value (initial) | Rationale |
|---|---|---|---|
| drivetrain.max_thrust 12 500 N × 16 wheels = 200 kN | powertrain.force 100 kN/side (200 kN total) | preserves 20° climbs / 30° stalls vs 559 kN weight | direct map |
| — | powertrain.power 260 kW/side | ~700 hp Tiger, split | new honest curve |
| — | powertrain.max_speed 10.5 m/s | 38 km/h Tiger | replaces implicit cap |
| — | powertrain.governor_gain, inertia | scaled from sandbox by mass ratio | feel start point |
| suspension.stiffness 551 613 N/m × 16 | support.stiffness_per_m ≈ 1.5 MN/m·m | same static sink (~5 cm) at 57 t over ~2×3.6 m contact | derived |
| suspension.damping | support.damping_per_m (scaled) | critical-ish | derived |
| drivetrain.lateral_grip 73 548 | MU 0.9 + LATERAL_GRIP_RATIO 0.55 (sim consts) | ellipse replaces linear | model change, tune in playtest |
| drivetrain.rolling_resistance | — (governor engine-brake) | — | feel dial changes; watch coast-down in playtest |
| suspension.ray_length/rest_length | — | — | no rays exist |

## 5. Landing order (each commit compiles + full suite green; no runtime forks ever)

- **A — shared terrain field** (`track/terrain.rs` in SimPlugin; view.rs consumes the shared
  resource). Behavior-neutral; server gains the field.
- **B — pure force core** (`track/forces.rs` extracted from model4; sandbox model4 becomes an
  adapter over it). Behavior-neutral in the sandbox — harness runs before/after must be
  bit-identical (`cmp`).
- **C — THE CUTOVER (one atomic commit)**: `track/sim.rs` + `TrackDrive` + protocol
  registration + spec swap (+ .tank.ron) + `src/driving/` deletion + consumer retargets
  (trace hash streams, debug force arrows → per-station contacts, net/diagnostics grounded
  count, headless_test contract asserts + system names, protocol test fixture, spawn.rs,
  model.rs requires) + view.rs belt source → TrackDrive + `tests/spherecast_scale.rs`
  deletion. The tree is never a cross-breed.
- **D — sandbox consolidation + docs**: models 1–3 deleted (the lab keeps ONE model — the
  promoted core — behind its rig/course/harness + both views); ADR-0025 written
  (supersedes 0005; 0006 amended: anchor restationed); architecture.md v4; HQ step 27;
  scripts/jitter+divergence field retargets; memory.
- **E — gates**: sandbox harness scenarios re-run (ramps, washboard, reversal, NEW steer
  scenario); headless determinism capture; MP smoke (server + client, drive + fire);
  feel-continuity checklist for Yan's playtest (climb limits, stop distance, pivot turn,
  slope hold, coast-down).

## 6. Risks + watch items

- **Steer feel is unproven** — the ellipse math existed but the sandbox never *played* steer
  (harness was straight-line). Mitigation: new harness steer scenario + Yan feel pass first
  thing after cutover. Turn radius / pivot authority tuned via `lateral_ratio` + `μ`.
- **Support along belt normal** (not ground normal) — honest model quirk, recorded in
  ADR-0025; watch on steep side-slopes.
- **Coast-down feel changes** (governor engine-brake replaces rolling_resistance dial).
- **Rollback cost**: force pass ~n_stations×3 columns×3 probes per side per replayed tick;
  with the broadphase field this measured ~66 µs/frame-side in the sandbox — fine, but
  rollback storms multiply it; cost trace covers it.
- **Protocol bump**: old clients refuse at handshake (fingerprint) — deploy server first.
