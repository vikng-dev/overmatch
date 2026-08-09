# Static friction as a first-class regime of the belt law — design brief

Status: SETTLED as the **per-element isotropic shear law**. This brief is the physics-derivation
record: the shear-modulus provenance, the elasto-plastic drift fix, the damping choice, and the
budget-on-elastic-load rationale below are the law's foundations and stand. Two things the brief
proposed were superseded on the way to the shipped law: the **single per-side aggregate resultant**
representation (`TrackGrip { sides: [[f32;2];2] }`, 16 bytes on the wire) gave way to the exact
**per-element strain field** (`TrackGripElements`; netcode is [[0027-element-grip-netcode]]), and the **anisotropic ellipse** (a 0.55 lateral share) gave way to
the **isotropic circle** (full μN in every direction; the `lateral_ratio` knob is deleted). ADR-0026 also records
the three places implementation falsified the draft (stiffness provenance, elastic vs damped
budget, the kinetic term removed from the grip regime).
Inputs: codex design deep-dive (`scratchpad/codex_friction_design.md` — regenerable), a
literature sweep (Dahl/LuGre/elasto-plastic, Janosi–Hanamoto terramechanics, tire transient
models, fixed-step numerics), an engine-practice sweep (solver friction internals, vehicle
sims, rollback prior art incl. Rocket League), and measured baselines (this repo, 2026-07-17).

## 1. The problem, measured

The shipped law is velocity-regularized Coulomb: `f = μ·load·clamp(slip/0.4)` on an ellipse.
Zero force at zero slip — a parked tank must creep to generate holding force. Measured (T-34
lab, 20° pad, zero command, 64 Hz):

- **Longitudinal park: ~98 cm/s hull creep, belts back-driven 0.74 m/s.** The slip law
  contributes only ~0.16 m/s; the rest is the GOVERNOR: any holding reaction at zero belt
  speed back-drives the belt through finite gain (engine = −G·belt). Hill-hold therefore
  requires BOTH ground stick and a drivetrain hold.
- **Side-slope park: ~30 cm/s**, belts still — the pure slip-law gap (lateral budget is
  0.55×, so sideways is ~1.8× worse than the old longitudinal estimate). This is what Yan
  felt: "should be in a much higher category of static friction."
- Tuning cannot fix it: the law is a damper (equilibrium under constant load is nonzero
  velocity by construction), and stiffening the slope hits the explicit-integration wall
  (creep < 1 mm/s needs ~50× past the 64 Hz stability limit). Independent confirmations:
  Drake/MuJoCo docs (velocity regularization "cannot hold static load"), TMeasy's published
  standstill artifact ("a vehicle standing on an inclined road would slowly slide down"),
  Bullet's raycast vehicle (force-level friction outside a solver — cannot park, slides in
  every engine backend).

## 2. Why not the alternatives (each rejected for cause)

- **Engine solver friction** (Avian contact constraints): the industry answer for rigid
  bodies (implicit velocity solve, cone clamp — "boxes rest on slopes", Catto). Rejected:
  solver warm-start/contact caches are not rolled back (prediction would bifurcate on
  replay), our belt stations are not solver manifolds (analytic field, not narrow phase),
  and the anisotropic ellipse/pressure-profile/terrain-μ future is not expressible there.
  This is the standing `Friction::ZERO` decision, reaffirmed.
- **Stateless implicit stick in our seam** (Jolt-vehicle pattern): compute the force that
  zeroes patch velocity, clamp to the cone. Works inside an iterating solver; in a
  force-before-integration seam it degrades to a one-tick-lag damper — residual creep = one
  tick of gravity (~5 cm/s on 20° for the Tiger). Better, not parked. Exact stateless static
  friction needs the full effective-mass/contact-coupling balance = a new contact solver.
- **Plain Dahl or LuGre single-state bristle**: structurally exhibits **presliding drift**
  (plastic flow at every deflection level — small oscillating loads below breakaway
  accumulate unbounded displacement; drift rate INCREASES with frequency), and plain LuGre
  is **provably non-passive** without a viscous term (Barabanov–Ortega: passive ⟺
  σ1 ≤ σ2·μC/(μS−μC) — with σ2=0 and any Stribeck dip it can generate energy). Our at-rest
  ~0.8 Hz suspension limit cycle + MG recoil are exactly the oscillating loads that would
  make a plain-Dahl tank walk downhill at rest.
- **Per-element bristle state** (97×3×2×2 floats): honest but ~4.7 KB/tank of
  force-affecting state; needs material-identity advection bookkeeping. Weighed here as the
  costly alternative to the aggregate — but this IS what shipped: the per-element field became
  the grip law, reconciled by wrench anchor + exact checkpoints rather than per-tick wire state
  ([[0027-element-grip-netcode]]).
- **`local_rollback` for grip state**: disqualified — parked means zero slip distance, so a
  stale local grip state NEVER heals (Dahl mismatch decays only with accumulated slip), and
  grip has no separate authoritative correction path. The one case being fixed is the one
  case local state cannot recover.
- **Gravity-share counter-force** (load_fraction·mg·tangent at zero slip): holds one test
  case, resists nothing else (external impulses, yaw torque), biases intentional drive,
  double-counts with the damper. A disguised hack; rejected.

## 3. The design (codex architecture + the literature's drift fix)

Three independent fields — controls (Dupont elasto-plastic), terramechanics
(Janosi–Hanamoto with elastic–plastic shear displacement: Altair's industrial track model),
tire dynamics (LuGre-tire deflection state) — converge on ONE object: **a saturating
elastic–plastic shear state per contact patch**. We declare it per track side.

### State

> The single per-side resultant below was the brief's proposal; it was superseded by the exact
> per-element field `TrackGripElements` (one strain per material link × column × side). The
> `TrackGrip` name survives as the summed derived telemetry over that field (hashed into `hblt`,
> off the wire). The elastic-load-proportional distribution, generalized-force (not world-anchor)
> character, and shared-ellipse cap described here carry over unchanged to the per-element law.

```rust
/// Per-side elastic grip resultant (N), [left, right] × [longitudinal, lateral/ρ].
/// Generalized forces, NOT world anchors — distributed through the existing per-column
/// contacts in load proportion (application points and lever arms unchanged).
pub struct TrackGrip { pub sides: [[f32; 2]; 2] }
```

The per-element field is root-resident local-rollback state that never crosses the wire per tick;
reconciliation is the wrench anchor + exact sparse checkpoints of [[0027-element-grip-netcode]].

### Law (inside `contact_side`, one law, no branches)

Per side: load-weighted slip resultant s̄ from the existing per-column slips; budget
C = Σ μ·load_i. The stored elastic resultant q updates by the **elasto-plastic Dahl** form
(backward-Euler rational update — self-bounded, ‖q‖ ≤ C, no clamp regime):

- **α = 0 (pure elastic, ZERO plastic flow) below the breakaway fraction** z_ba ≈ 0.5·C —
  this is Dupont's amendment and the drift killer: below breakaway the state is a real
  spring, oscillating loads cannot ratchet it. (Codex's minimal Dahl lacked this; the
  literature says that variant walks downhill under vibration. Amendment adopted.)
- α blends smoothly to 1 approaching the cap; at the cap, sustained slide converges exactly
  to today's saturated kinetic ellipse.
- Per-contact force: g_i = P₁(q/C + κ(s_i)) — the elastic term and today's kinetic
  regularizer SHARE the one ellipse cap (cannot double grip). With q = 0 the law is
  bit-identically today's. C0 across stick/slide and contact gain/loss (projection to a
  shrinking budget, no resets).

### Belt-hold (closes the governor leak)

h = H(|target|/vs)·H(|belt|/vs) (smoothstep, the existing 0.4 m/s scale):
`ḃ = (engine − (1−h)·R)/I` — at zero command + zero belt speed the locked drivetrain bears
the ground reaction instead of being back-driven; during motion h→0, today's dynamics
unchanged. Legitimate force balance: the belt's 1-D coordinate is fully known to the belt
integrator, which is `transmission::step` — the Governor arm calls `forces::governor_belt` (this
term, verbatim), and the declared-gearbox arm builds the same hold from `forces::hold_blend`. A
future neutral/clutch or brake-damage mechanic weakens this term explicitly.

### Constants (declared, with provenance)

- K (bristle stiffness) from a DECLARED park target: 30° longitudinal park within 5 mm
  shear → K = (W·sin30°/2)/0.005 ≈ **28 MN/m per side** (Tiger). 20° equilibrium: 3.4 mm
  long / 6.2 mm lat.
- **Terramechanics identity**: K_bristle ≡ C/j_K — our 5 mm target corresponds to a
  Janosi–Hanamoto shear modulus j_K ≈ 9 mm = FIRM SOIL (Wong: clay 6 mm … loose sand
  25 mm; rubber pads on firm ground 75 mm). The bristle stiffness IS the terrain shear
  modulus in disguise — the future ground-type mechanic gets a physical dial, not a feel
  hack. (Wong's rubber-pad value would mean ~8 cm of settle-back on a 30° park — a softer,
  arguably more "real" feel; start at the declared 5 mm, expose via terrain later.)
- Stability at 64 Hz: hard semi-implicit cap k ≤ 4·m_eff/dt² ≈ 1.7×10⁸ (margin 6×);
  Catto design rule (≤16 Hz bristle mode) ≈ 1.0×10⁸ (margin 3.7×); coupled belt–hull mode
  ωΔt ≈ 0.82 vs limit 2. The rational update is dissipative by construction (the
  literature's endorsed fixed-step family: implicit/exponential/clamped).
- Lateral capacity limit falls out: arctan(μ·0.55) ≈ 26.3° — steeper pure side-slopes
  slide BY DESIGN (budget decision, not stiffness defect).

### What we deliberately do NOT adopt

- **Isotropy (no ellipse, full μN laterally)** — the brief deferred this as the soft-soil-era
  alternative, wary of repealing the `lateral_ratio` pivot dial and re-tuning the steering gates.
  The settled law took it: the per-element cap is an isotropic **circle** (no lateral share),
  which is what re-tightened the steering gates. Wong & Chiang applied it as an aggregate
  resultant; the shipped law applies it per element.
- Full LuGre (Stribeck curve, bristle-rate damping): unnecessary complexity + the
  passivity trap.
- Sleep as the hold mechanism: stays available as a later bandwidth/bit-exactness lid
  (Fiedler-style server-owned at-rest bit, never engine island sleep — lightyear disables
  Avian sleeping for rollback, Avian has open wake bugs #654/#901).

## 4. Gates (all pre-agreed before implementation)

1. **Hill-hold**: all four slope poses, zero command, 30 s: fall-line displacement < 1 cm;
   final-second speed < 1 mm/s; **belt phase moves < 1 cm** (the track is not quietly
   rotating under a held hull).
2. **Drift (the elasto-plastic differentiator)**: parked on the pad with an oscillating
   disturbance (washboard idle / scripted ±5% load wobble at ~1 Hz, 60 s): net displacement
   ~0 (a plain-Dahl implementation FAILS this gate — it must be in the suite to prove the
   α-branch works).
3. **Clean drive-off**: park → command ±0.5: grip sign follows command without chatter
   after 2 yield-travels; hull reaches 0.1 m/s within I/G + C/(K·vs); no sticky-launch
   feel artifact.
4. **Steering regression**: the full existing analyze_steer suite (pivot/turn/slalom/
   monotonicity/ellipse/dissipation) — low-speed pivot feel is the main regression risk
   (sub-saturation slip now builds resistance instead of the weak creep branch).
5. **Rollback**: the element field (`TrackGripElements`) in the canonical hash; slope-park replay
   bit-identical; solo MP slope parks ≈ zero post-settle rollbacks; injected grip mismatch
   reconciles without a park/correct/park loop.
6. **Parity** (historical): a `q ≡ 0` grip-disabled switch let the strain regime land bit-identical
   to the pre-grip baseline during bring-up. That parity switch and the pre-grip baseline no longer
   exist — the strain law is the only grip law — so this gate is retired, not live.

## 5. Failure modes to watch (from all three inputs)

Prediction corrections fighting stick (threshold too tight → repeated TrackGrip-attributed
rollbacks); remote parked pose jitter (measure confirmed pose vs render-error separately);
low-speed steering too resistant (the creep branch is gone — deliberate, but feel-check);
curb / one-track-contact behaviour (the aggregate's insufficiency here was the evidence that
justified the per-element field that shipped); ledge-transition traction pulse (memory clips to a shrinking
budget — continuous but watch it); coast-down capture feel through the h-blend band.
