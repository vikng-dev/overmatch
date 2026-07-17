# Static friction: an elastic–plastic strain regime inside the belt law

Tanks hold and grip through a **per-side elastic–plastic shear strain state** — the
Janosi–Hanamoto form from tracked-vehicle terramechanics, carried as `TrackGrip` (4 floats:
`[left,right] × [longitudinal, lateral/ρ]`, generalized force resultants, never world
anchors). Ground shear accumulates strain at the declared stiffness; force = the strain
resultant distributed through the tick's contacts in elastic-load proportion, capped per
contact by the same μ/lateral-ratio ellipse as always; past the cap the Dahl return term
saturates the state at the budget — sustained sliding converges to exactly the saturated
kinetic ellipse. Below half the budget the state is a pure spring (Dupont's elasto-plastic
branch — zero plastic flow, so oscillating loads cannot ratchet it downhill: the documented
presliding-drift artifact of plain Dahl/LuGre). A smoothstep **belt-hold** lets the locked
drivetrain bear the ground reaction at zero command + zero belt speed, closing the measured
governor back-drive leak (0.74 m/s of a parked tank's ~1 m/s longitudinal creep was the
belt itself being cranked backward through finite governor gain).

Stiffness is DECLARED, not tuned: `μ·W/2` per side developed over one shear modulus —
**Wong & Chiang's measured 75 mm for rubber track pads on firm ground**. A 20° park settles
~28 mm before holding. When the ground-type mechanic lands, terrain owns this dial (firm
soil ~10 mm, loose sand 25 mm). Supersedes the "per-element bristle someday" note in
[[0025-belt-force-locomotion]]; the retired brush anchor ([[0006-static-friction-brush-anchor]])
stays retired.

## What implementation falsified in the approved design (recorded honestly)

1. **The 5 mm park-shear target was numerically wrong at 64 Hz.** It drove a full-amplitude
   coupled roll/yaw limit cycle (measured: period-2 side swap, 7× per-tick load swings).
   The Wong modulus is 8× softer and sits every mode deep inside the stability region
   (ωΔt ≈ 0.27) — and is the better-provenanced number anyway.
2. **The grip state must key on the ELASTIC support load, not the damped actual.** The
   support damper converts mm-scale pose wobble into ±90 kN load transients; feeding those
   into an integrating state amplifies noise into force oscillation. Coulomb budget follows
   the sustained weight-bearing force. (The kinetic-only law still uses damped load,
   unchanged — the `grip=off` parity switch reproduces the pre-grip sim bit-for-bit.)
3. **The velocity-regularized kinetic term is ABSENT from the grip regime.** Its slope near
   zero slip (μN/0.4 ≈ 270 kN·s/m per side) is an explicit damper at the 64 Hz stability
   margin — a latent marginal instability the old sim never exposed because creep kept slip
   in the saturated flat (dF/dv = 0). The strain state IS the force law (per Rill's
   published eigenvalue argument for deflection-state tire models); a small σ1 viscous
   partner (ζ = 0.15 on the load-weighted slip) damps at-rest ringing.
4. The sandbox adapter read `GlobalTransform` (frame-rate pose) — the last "game-illegal
   habit"; multi-tick frames probed terrain against stale hulls. Now tick-truth
   `Position`/`Rotation`, same as the game.

## Consequences

- **Parked tanks hold**: 30 s on 20° in all four orientations = 0.00 mm drift, 0.000 mm/s,
  belt phase frozen, load noise ≤ 1 N. Drive-off from a held park reaches 0.1 m/s in
  < 0.1 s, no stick chatter. Pure side-slopes above arctan(μ·ρ) ≈ 26° still slide — budget,
  not bug.
- **Driving grip is FULLER below saturation** (the honest consequence of strain-based
  force): measured turn radii tightened 2–6× (34.8/15.2/5.0 m at steer .1/.2/.3 vs
  93/35/18 before — now near the no-slip kinematic radii) and pivot rate doubled
  (4.87 rad/s lab vehicle). Force direction lags slip by ~one shear modulus of travel
  during fast slides (physical relaxation). **`LATERAL_GRIP_RATIO` is the dial** to
  re-heavy steering if the feel pass wants Wong/Merritt turning resistance back.
- Netcode: `TrackGrip` replicates + predicts + rolls back (LinearVelocity pattern,
  16 bytes, own 2 kN threshold, `hblt` hash stream); PROTOCOL_REV 13.
- The determinism/parity discipline held throughout: `grip=off` (stiffness 0) is
  bit-identical to the pre-grip baseline — the entire regime, belt-hold included, is one
  multiplicative gate away from the shipped phase-B law.
