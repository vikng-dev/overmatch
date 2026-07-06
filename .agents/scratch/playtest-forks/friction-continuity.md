# F4 — Friction force-law continuity (hard stick gate vs blended band)

**Status:** settled 2026-07-06 · Yan's SP + MP feel drive PASSED ("feels pretty good": hill-hold
release, coast-down capture, pivot feathering all read right). Residual MP instability observed in
the drive (ramp nose-dive → hull catches → never settles, continuous jitter) is the hull-contact
replay machine (client `hc=0` on replayed ticks — see the 80/10 paragraph below), NOT the friction
law; next slice.

**Correction (2026-07-06, architecture-review session):** the lat0 blend numbers below are
**bimodal**, not stable — 4 additional runs measured 1 / 63 / 1 / 50 rollbacks, the high-count
runs still escaping to 5–35 m runaways. Both the runaway's persistence and the tiny "1 rollback"
counts are artifacts of a diagnosed lightyear 0.28 check-starvation bug: at LAN/loopback latency,
`balanced()` input delay yields zero prediction margin, the receive-time mismatch check skips
every confirmed update (`confirmed_tick >= current_tick`, strict, never retried), and state
rollback goes silently dead — divergence then runs unchecked. Fixed by the rollback watchdog
(`src/net/watchdog.rs`, this branch). Until/unless that context changes, **lat0 rollback counts
from pre-watchdog builds are not a valid A/B metric** (they measure check starvation, not
convergence); lat0 |Δp| tick-row divergence remains valid.

**The question.** The drive/friction model's static↔kinetic hand-off: should it stay the original
**hard gate** (a wheel grips below `STICK_SPEED = 0.3 m/s`, slips above — anchor planted/dropped as
a binary `Some`/`None`, force law switched whole), or the new **blended band** (a smoothstepped
static fraction `w_static` across ±40 % of the stick speed, the anchor permanently planted while
loaded and relaxed toward the LuGre kinetic steady state in proportion to slide)? The hard gate is
the same binary-transition class the sphere-cast suspension removed: under MP prediction, mm/s-scale
cross-machine velocity noise near the threshold flips the force *law* — the client and server
apply different regimes at the same tick and diverge every tick they straddle it.

**Default — blended band (the code as of this slice).** Three coupled changes in `apply_drive`:
1. `w_static = 1 − smoothstep` across `[STICK_SPEED·(1−STICK_BAND), STICK_SPEED·(1+STICK_BAND)]`
   (0.18–0.42 m/s): every static-vs-kinetic force pair (longitudinal hold vs coast, lateral hold vs
   skid grip) is a `w_static` blend instead of an `if gripping` switch.
2. The brush anchor never flips at the speed gate. It stays planted while the wheel bears load and
   relaxes toward the **LuGre kinetic steady state** — the bristle trailing on the friction
   ellipse, deflected along the slide so its spring opposes it (`z_ss = sign(v)·g(v)/σ0`) — at rate
   `(1 − w_static) · ANCHOR_RELAX_RATE`. The trail deflection is CONTINUOUS in velocity (per-axis
   `v/v_ref` clamped at the ellipse semi-axis, `v_ref` = band top), not `v̂`-scaled — the unit
   vector is discontinuous through v = 0 and its near-band direction churn teleports the target
   across the ellipse. Frozen at rest (`w = 1`, bit-identical to the old planted anchor). Releases
   only on load loss (airborne/unloaded), as before.
3. The ellipse cap still never resets the anchor (the documented stick-slip invariant, untouched).

**Chosen because** it kills the powered-wedge chaos machine at the source, measured on the beached
slab-edge repro (`SPIKE_SPAWN_POSE` beached pose + `SPIKE_SIM_REVERSE` + `SPIKE_SIM_LONG`):
at lat0 the baseline diverges 44 rollbacks → a 50 m runaway silent desync (the client tips off the
edge and drives away while the server stays wedged); the blend holds both ends wedged together —
1 rollback (spawn settle), |Δp| p50 2.2 mm, residual silent window 0.43 m max (117× smaller; the
"mismatch ignored by check" class is pre-existing). At 80/10: 207 → 94–156 rollbacks (chaotic
band), |Δlv| p50 0.58 → 0.15 m/s, |Δp| p50 19 → 4 mm; the residual storm is the client's hull-edge
contact failing to re-form during depth-8 prediction (client hc=0 vs server hc=2 — a
contact-replay machine, not friction). Washboard lat0 7→7, 80/10 within the measured chaotic band
(1–147); flat-cruise bit-exactness preserved (0.000 across all fields, ticks 650–1350); drop test
clean (min py −0.042); the 20° ramp park is *stronger* (1.5 cm creep vs the baseline's 0.6 m
slide-then-catch) because capture through the band starts at Coulomb strength.

**Known tradeoff (2026-07-06 review).** The fully-static hold region narrowed from [0, 0.30) to
[0, 0.18) m/s: mid-band (0.18–0.42) the hold is w_static-weighted and the anchor deflection bleeds
toward the LuGre kinetic steady state, so maximum sustained resistance there is kinetic-strength,
not the old full-Coulomb capture at any speed < 0.3. A tank on a slope near the traction limit
(steeper than the tested 20° ramp) nudged to ~0.25 m/s by an impact may settle into a creep where
the old gate re-parked it. Dial if this ever reads wrong in combat: narrower `STICK_BAND` (crisper
capture, less noise immunity). Not observed in play; recorded from code review.

**Alternatives kept alive.**
- **The hard gate** (baseline): `git revert` of this slice's `driving.rs` change restores it
  whole — the old code had no knobs to preserve.
- **Narrower/wider band**: `STICK_BAND` (0.4) is the feel dial. Narrower → crisper hill-hold
  release, closer to the old snap, less noise immunity; wider → mushier hand-off, more immunity.
  `ANCHOR_RELAX_RATE` (1.0) is the second dial: lowering it (~0.25) trades capture strength for
  extra smoothing of contact-velocity noise.
- **Three rejected relax variants** are documented at the anchor-update comment in `apply_drive`
  (relax-toward-CONTACT leaked the hold deflection and lost the ramp park, 7 m slide; NO relax
  re-gripped from a stale saturated spring, washboard coast-down 1→32 rollbacks; a `v̂`-scaled
  saturated target teleported under near-band direction noise, wedge storm re-armed 1→48) —
  measured dead ends, not live alternatives.

**Why it's a playtest call.** The blend changes three low-speed feels that can't be settled at the
desk: (a) **hill-hold release** — pulling away from a slope park now eases through a ~0.24 m/s
blend window instead of snapping loose at 0.3 m/s; (b) **coast-down capture** — a released tank
decelerating through the band is grabbed *harder* than before (Coulomb-strength trail vs a fresh
zero-deflection spring), so the last half-second of glide shortens and the park feels firmer —
watch whether that reads as "planted" or "sticky"; (c) **low-speed creep/pivot** — feathering the
throttle near the stick speed now rides a partial static spring instead of toggling regimes, which
should kill micro stick-slip shudder but may read as drag. SIM-AFFECTING both ends identically —
no env fork; this is the third slice of an accepted campaign, so the old behavior is preserved in
git, not behind a switch.

**Revert cost.** `git revert` of the `src/driving.rs` change (one commit once landed). No spec
sheet, protocol, or asset coupling; `STICK_BAND`/`static_weight`/the anchor-target block are the
whole surface.

**Lives in.** `src/driving.rs` — `STICK_SPEED`/`STICK_BAND`/`ANCHOR_RELAX_RATE` (the dials),
`static_weight` (the blend), the anchor-target block in `apply_drive` (the LuGre steady-state
relax + the three rejected variants), the blended `f_fwd`/`f_lat`. Repro + numbers: the powered-wedge harness levers
(`SPIKE_SIM_FORWARD` added this slice, `SPIKE_SIM_REVERSE` + beached `SPIKE_SPAWN_POSE` is the
storm), `ancm` per-wheel anchor bitmask in the tick trace (`src/trace.rs`).
