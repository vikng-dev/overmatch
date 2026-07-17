# Belt-force locomotion: the sandbox track model is the drive sim

Tanks drive on the **track model's belt forces** — the field-belt model developed and
feel-proven in the track sandbox, promoted wholesale. Per side, the closed rest pin-line loop
(the same `build_route` circles the view's feasibility gate uses) is resampled into
pitch-spaced force stations advected by belt travel; each ground-facing segment probes the
analytic terrain field across three lateral collocation columns (Simpson weights over the shoe
width), takes a clipped-linear pressure profile, and applies **support** (penalty spring along
the belt's inward normal, normal-velocity damped, engagement-ramped) and **traction**
(slip-saturated friction ellipse: longitudinal slip vs belt surface speed, lateral scrub, both
budgeted against μ × load with a lateral grip ratio). Longitudinal reactions accumulate into
**belt dynamics**: a constant-power engine curve (P/v, force-capped), a governor chasing
`command × max_speed`, reflected inertia, and phase advection. Steering is the differential
command `throttle ± steer` per side; yaw torque emerges from traction lever arms — nothing
scripted. One force law, one implementation: `src/track/forces.rs` is pure math (no ECS),
consumed by the game adapter (`src/track/sim.rs`) and by the sandbox — bit-identical by
construction and proven by harness `cmp`.

This **supersedes [[0005-raycast-roadwheel-locomotion]]** (the raycast placeholder and its
per-wheel spring/friction stations are deleted — hull support now comes from the belt itself)
and **retires [[0006-static-friction-brush-anchor]]** (the brush anchor was a patch on the
raycast model's friction; the owner's call on cutover: nothing structural carries over from
the placeholder, including its hold machinery).

## Considered Options

- **Keep the raycast sim under the new track view** (cross-breed) — shipped briefly in phase
  A; rejected by owner mandate: two locomotion models is a fork, and the raycast feel was
  never the target.
- **Transplant the brush-anchor hold into the belt model** — attempted during the cutover and
  reverted: it dragged five tuned constants and old-feel-derived powertrain numbers into the
  clean model. If playtest wants hill-hold, it arrives as a **per-element bristle** extension
  of the slip law itself (static friction as physics inside the ellipse budget), not as a
  bolted-on anchor.
- **A/B runtime switch for rollback safety** — cancelled; parity evidence comes from the
  bit-repeatable sandbox harness, the contract tests, and playtest. No conditionals ship.

## The netcode contract

`TrackDrive { throttle, steer, sides: [{speed: f32, phase: f64}; 2] }` is sim state on the
tank root: owner-predicted, replicated, rolled back (the `LinearVelocity` registration
pattern — float-threshold rollback condition, wire-surface pinned, `hblt` determinism-hash
stream). The shaped command lives here (slewed via `INPUT_RAMP`) so every tank responds with
the same feel regardless of input device cadence. `phase` is `f64` and unbounded — it advects
the force stations, so it is authority state, and the view derives its exact scroll from it
(remote tracks get true link phase, no client integration drift). `TrackContacts` is
telemetry only: never hashed, never rolled back.

## Consequences

- **Support acts along the belt normal, not the ground normal** — honest model quirk; watch
  steep side-slope behaviour.
- **No bump-stop cap** — deep penetration resolves through the pressure profile alone.
- **Slope-parking creep is accepted**: the clean slip law has no static friction, so a parked
  Tiger creeps ~16 cm/s on a 20° slope (bounded equilibrium — the ellipse prevents runaway).
  The recorded fix, if playtest demands one, is the per-element bristle above.
- **Coast-down feel changed**: governor engine-brake replaced the rolling-resistance dial.
- **Surface friction is a sim constant** (μ 0.9, lateral ratio 0.55, slip saturation 0.4) —
  ADR-0007-bucket-3 material: a property of the track–ground *pair*, reserved for the future
  terrain/ground-type mechanic, deliberately not vehicle spec. Vehicle spec (`.tank.ron
  track:`) carries only `powertrain` + `support`, scaled from sandbox tuning by mass ratio.
- **Vehicle collision proxies run `Friction::ZERO` (min-combine)** — all grip is the model's;
  Avian's default contact friction would silently add unmodeled hold.
- **`Drive` capability gates the command, not the contact model** — a dead engine still
  grips and holds; it just cannot thrust.
- **Cost**: ~stations × 3 columns × 3 probes per side per tick, analytic field only (no
  spatial queries, no BVH rollback dependency) — measured ~66 µs/side in the sandbox;
  rollback storms multiply it, covered by the cost trace.
