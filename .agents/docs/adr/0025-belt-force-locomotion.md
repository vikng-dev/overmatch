# Belt-force locomotion: the sandbox track model is the drive sim

> **Status: the locomotion model stands; the netcode bullet (owner-predicted, rolled back) is
> superseded by [[0037-one-authoritative-timeline-and-view-overlays]]** (2026-08-15): belt state
> replicates on the one timeline, nothing predicts it. **How the belt meets the world is settled by
> [[0041-track-contact-channels]]**, which replaces the contact geometry here.

Tanks drive on the **track model's belt forces** — the field-belt model developed and
feel-proven in the track sandbox, promoted wholesale. Per side, the closed rest pin-line loop
(the same `build_route` circles the view's feasibility gate uses) is resampled into a fixed count
of force stations advected by belt travel; each ground-facing segment reads one height-grid facet
plane per lateral collocation column (three, with Simpson weights over the shoe width), takes a
clipped-linear pressure profile, and applies **support** (penalty spring along the surface
normal, normal-velocity damped, engagement-ramped) and **traction**
(per-element isotropic shear: each grounded link accumulates a directional strain whose force
saturates against an isotropic μ × its own elastic load — one circular budget, no friction
ellipse and no lateral grip ratio; see [[0026-static-friction-strain-regime]] /
[[0027-element-grip-netcode]]). Longitudinal reactions accumulate into
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

- **Support acts along the surface normal**, and a stop collider per side caps the travel; a piece
  under a belt leaves this channel ([[0041-track-contact-channels]]).
- **Slope-parking hold shipped**: the per-element elastic–plastic strain law (the "per-element
  bristle" foreseen in the options above) landed as the grip law — a parked Tiger holds on grade
  rather than creeping. See [[0026-static-friction-strain-regime]] and the settled per-element
  contract [[0027-element-grip-netcode]].
- **Coast-down feel changed**: governor engine-brake replaced the rolling-resistance dial.
- **Surface friction is a sim constant** (μ 0.9, slip saturation 0.4 — isotropic, no lateral ratio) —
  ADR-0007-bucket-3 material: a property of the track–ground *pair*, reserved for the future
  terrain/ground-type mechanic, deliberately not vehicle spec. Vehicle spec (`.tank.ron
  track:`) carries only `powertrain` + `suspension`, scaled from sandbox tuning by mass ratio.
- **Vehicle collision proxies carry their declared substance's friction**, and a stop collider
  the track's; support and traction on the ground are the model's.
- **`Drive` capability gates the command, not the contact model** — a dead engine still
  grips and holds; it just cannot thrust.
- **Cost**: 48 stations × 3 lateral points per side per tick, one facet lookup per column; solids
  are found by spatial query. MEASURED 2026-11-08: the belt stage is 0.51 ms for 30 tanks locally.
