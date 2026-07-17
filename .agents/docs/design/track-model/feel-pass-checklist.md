# First-drive feel pass — the belt sim (phase B)

For Yan's first in-game drive on the new locomotion (commit-E gate; codex steer review §3
distilled + our measured gate numbers). Two lists: differences to EXPECT (surprise ≠ bug),
and behaviours that are BUGS no matter how they feel. Then the tuning dials.

## Expect these differences (pre-warned, by design)

- **No hill-hold.** Parked on 20° the tank creeps DERIVED ~0.16 m/s along-slope, ~0.29 m/s
  on a pure side-slope (the lateral grip budget is 0.55× — side-slope creep is ~1.8× the
  longitudinal number, not the same). Bounded equilibrium, not runaway. Accepted gap; the
  fix, if wanted, is the per-element bristle (ADR-0025).
- **Throttle onset feels layered.** The same 4.0/s input slew as before, but it now feeds
  belt inertia + a governor instead of direct thrust — key-tap response can feel slightly
  elastic. The slew itself is unchanged.
- **10.5 m/s is a belt-speed target, not a hull-speed cap.** Hull speed sits below it under
  slip, turn scrub, or incline. DERIVED: power-limited climb speed on 20° is only ~2.7 m/s —
  "climbs 20°" means crawls up it, "stalls 30°" (gravity demand ~280 kN vs the 200 kN cap).
- **Coast-down shape changed.** Old: force ∝ speed (rolling-resistance dial). New: governor
  engine-brake — power-limited when fast, much stronger as speed falls. Expect a longer
  high-speed glide, then a decisive low-speed stop.
- **Turning costs forward speed.** Lateral scrub and longitudinal traction share one
  friction ellipse. MEASURED (T-34 lab): pivots are snappy (±144°/s, but that's the lab
  vehicle) while at-speed turns run wide — a 0.4-steer turn at 5.4 m/s fits a ~53 m circle
  vs ~11 m no-slip. If turns feel sluggish at speed, that's the ellipse, and it's a DIAL.
- **Steering saturates near full throttle.** The mixer clamps each side separately: at full
  throttle, steer only slows the inner track. Nonlinear by design.
- **Support acts along the belt plane, not the terrain tangent** (ADR-0025). Ramp
  transitions and side-slopes can feel different from the old wheel rays.
- **No bump stop.** Hard landings resolve through the pressure profile alone.

## Treat as bugs (report immediately)

- Wrong yaw sign (steer right must turn right).
- Noticeable left/right asymmetry on flat ground (gates measure 0.03%).
- A pivot that translates significantly or can't establish yaw.
- Turn radius NOT tightening as steer increases (below saturation).
- One track consistently losing contact on flat-ground turns.
- Slope steering reversing sign, or slope creep accelerating without bound.
- Prediction/replay artifacts specific to steering (yaw snap on rollback while the hull
  eases — the phase/render-error seam; conspicuous ≠ acceptable, capture it).

## Dials (symptom → first knob; all in `tiger_1.tank.ron` unless noted)

| Symptom | Dial | Direction / collateral |
|---|---|---|
| Belt response / pivot entry lazy | `powertrain.inertia` | Lower = faster belt reversal; more wheelspin harshness |
| Belt chases command weakly; release-braking soft | `powertrain.governor_gain` | Raise; not a turn-radius dial |
| High-speed accel / uphill speed weak | `powertrain.power` | Raise; no effect at stall (force cap binds) |
| Can't pivot or climb from rest | `powertrain.force` | Raise; may just add slip if traction saturates |
| Pivot reluctant / turns too wide | `LATERAL_GRIP_RATIO` (sim const) | Lower; less yaw damping, more side-slope creep |
| Fishtails / pivot too loose | `LATERAL_GRIP_RATIO` | Raise; wider turns |
| Everything slips; braking/climb weak | `MU` (sim const) | Raise; changes everything together |
| Low-speed creep / mushy contact | `SLIP_SATURATION` (sim const) | Lower stiffens; sharper gradients, harsher corrections |
| Keyboard onset wrong, steady turn fine | `drive::DRIVE_SLEW_PER_SECOND` | Only after traces separate shaping from belt transient |
| Bounce / contact loss in turns | `support.*` | Only if per-side load telemetry proves contact loss |

Keep `max_speed` fixed through the first pass — it's the one clean continuity anchor. No
yaw-damping dial exists and none should be added blind: distributed lateral scrub already
yaw-damps (measure first — codex steer review §5.6).
