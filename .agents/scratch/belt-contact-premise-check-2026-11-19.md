# Belt contact premise check

**Decision: round 1's architecture is right, but its per-point exact Newton solve is not the minimum.** Keep the drivetrain-response metric and the exact owner re-solve inside the contact row. Replace the 64-iteration Newton, the latched regime switch and the cached face path with a capped two-step row that falls back to bare belt inertia. Nothing here was built or run; every number is DERIVED unless marked MEASURED.

## 1. The physical truth

A belt is a closed loop with one scalar freedom (its speed along its path) and its own inertia. It is tied to the hull only through the drivetrain: a brake (bounded, static above kinetic), a clutch (bounded, reflecting crank inertia when locked), steering, and the engine's crank budget. A rigid contact on its face exchanges one friction impulse, bounded by the Coulomb disk, between the other body, the hull and the belt coordinate. The contact must feel the belt as the drivetrain presents it at that instant: immovable while a brake holds within capacity, heavy while a clutch is locked, light when free. It must never take more momentum than that inertia and those bounded elements can supply in the interval, never receive a second budget, and never add energy. It need not be solved exactly per point, because nothing else in Avian's one-iteration-per-substep relaxation is.

## 2. The alternatives

One number decides most of this. Round 0's creep is not a tuning problem; it is the hull-to-belt mass ratio.
- With a free belt, the contact drives hull slip to the belt's mid-substep speed `p/I`, not to zero, and the brake then erases only the belt's motion.
- The steady state is `v = g·sinθ·h / (W·I)`. With the lens figures (W ≈ 3.5e-5 /kg, I ≈ 4750 kg, so W·I ≈ 0.166) this reproduces the MEASURED creep to about 3%:

| Substeps / slope | DERIVED m/s | MEASURED m/s |
|---|---|---|
| 6 / 15° | 0.0398 | 0.0390 |
| 6 / 30° | 0.0769 | 0.0761 |
| 3 / 15° | 0.0796 | 0.0787 |
| 12 / 15° | 0.0199 | 0.0192 |

Any formulation where the contact row sees bare belt inertia removes only W/(W+1/I) ≈ 14% of slip per visit.

**a. Belt as solver bodies with Avian joints: rejected.**
- Avian 0.8.0 joints are XPBD and solve in their own pass after contact relaxation (`vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/xpbd/plugin.rs:33-42`). That is "bare inertia, then a separate brake solve", which is round 0's creep by construction.
- It offers distance, fixed, prismatic, revolute and spherical joints. Motors have `max_torque`/`max_force` clamped per substep (`xpbd/joints/revolute.rs:378-380`). There is no joint friction, no static/kinetic pair, no gear or differential joint, and no aggregate crank limit.
- A collider's surface velocity is a per-manifold constant (`contact/mod.rs:87`); it cannot follow a body's joint coordinate without a fork hook equivalent to today's.
- A rigid body's surface velocity is ω×r, which matches a wheel, not a stadium-shaped belt run.
- Fork grows (gear joint, surface-follows-body) and the existing block solver would be duplicated. The sampled ground channel never enters the solver and already feeds that block.

**b. Bounded rows as peers in the relaxation: rejected on convergence.**
- It is unconditionally energy-safe, cannot panic, is reversible, and is about 60 lines.
- Its fixed point is exact only with every row warm-started. A fresh contact converges at 0.86 per visit, 0.74 per substep.
- With friction cleared each substep (the lane's slab witness) it creeps at roughly 3.8·g·sinθ·h ≈ 0.02 m/s, scaling with substep length. In play that scales with contact churn, which is highest on rubble.
- It would be the right answer if belt inertia were comparable to hull effective mass. It is about one sixth.

**c. Effective mass per owner, bounds checked after: recommended, in the form below.**
- A single linearised step with the held (zero-mobility) metric is unsafe when a bound releases mid-step. A hull arriving at 5 m/s on locked brakes would hand the belt (s/W − cap)/I ≈ 30 m/s, the original defect class.
- So the "second pass if a bound was exceeded" is mandatory, not optional. Done properly it is round 1's metric without round 1's iteration.

**d. Keep round 1 and fix the P2s: workable, not minimal.**
- The reachable panic, the duplicated regime law, the order-dependent latch and the per-point cost all come from one choice: requiring each point to converge to 1e-9 on a piecewise metric (`src/avian_ext/surface_motion.rs:525-561`).
- Fixing them individually adds code: continuity repair in `FaceResponse::apply`, a complementarity pass for the regime, an allocation-free cache.

| | No energy creation | Holds within capacity | Slides beyond | Order sensitivity | Panic on valid input | Block solves per point visit | Lane production lines | Fork | `avian_ext` boundary |
|---|---|---|---|---|---|---|---|---|---|
| a | yes | no, creeps like round 0 | yes | joint pass order | no | n/a | more, drivetrain rebuilt as joints | grows | clean |
| b | yes | only at warm fixed point | yes | none at convergence | no | 1 | about 1,000 | same | clean |
| c (recommended) | yes, by fallback | yes, one visit | yes, exact in two steps for one release | none: regime is a function of totals | no | 1 typical, 3 worst | about 1,190 | same or −10 | clean, via trait |
| d | yes | yes | yes | latch remains unless reworked | yes until repaired | 4 or more, with allocation | about 1,720 plus repairs | same | cycle unless moved |

Cost scales with tanks × bound points × 2 passes × substeps, and runs serially when a colour holds under 64 constraints (`solver/plugin.rs:791,887`). Round 1 MEASURED about 8 µs per bound point per pass (local, opt-level 1); the recommended row is about a quarter of that by solve count, unmeasured.

## 3. Recommendation and build outline

**The row** (finite-surface contact point, one visit):
1. Build the metric from the rigid terms plus each owner's current-face mobility for that coordinate. Cache the response per owner and recompute it only when the face changes.
2. Minimise on the Coulomb disk with that fixed metric, using `disk_minimum` as it stands (`surface_motion.rs:568`).
3. Apply the impulse to both bodies, add it to the owner's contact total, and re-solve the owner's block exactly from the substep-start state (`MechanicalRows::solve`, `src/track/transmission.rs`).
4. If the face is unchanged, accept. If it changed, take one corrective step from the landed point with the new face's metric.
5. If the face changes again, roll back to the base and take the step with bare belt inertia. That metric is never smaller than the true one, so the step undershoots and cannot add energy. Relaxation carries the remainder, as it does for every Avian row.

**Decisions inside it:**
- **Regime owner:** `block::solve_limited` alone (`src/track/block.rs:405-517`), with `row.dynamic` left intact and re-decided on every solve from the substep-start state and the current totals. No latch in the seam. The committed belt state is then a function of final contact totals, so contact order cannot decide whether a brake holds.
- **Known consequence:** when the hull's demand exceeds static capacity but the sliding answer lies between kinetic and static, no self-consistent point exists. The row lands on the fallback, and the effective brake force sits between the two ratings. I judge that acceptable; the latch was the alternative and it is what made the result order-dependent.
- **Boundary:** `avian_ext::surface_motion` defines a trait (speed, current mobility, free mobility, push impulse returning whether the face changed). `track` implements it for the drivetrain; a replica gets a named prescribed implementation. `avian_ext` stops importing `track`.
- **Biased-pass friction:** drop it for belt rows and let them run in relaxation like stock friction. The witness for it clears cached friction every substep, which play never does. This is separable; keep it if the slab witness must stay in its cleared form.

**Deleted from the lane (about 635 lines):**
- `ContactMetric::minimum`'s loop and line search, `Evaluation.potential`, `trial`, and the dense free mobility (about 100 lines).
- `failed_static`, `release_static`, `break_static`, `settled` and the `sliding` field (about 110).
- `FaceResponse::apply`, the cached branch, and `Solution.{multipliers, equalities, snap_basis}` (about 100).
- `MechanicalRows::potential` (15).
- The settled re-install path in `collect_surface_speeds` (35).
- The duplicate KKT assembly, folded into one `face_system` (20).
- The remainder of `surface_motion.rs`'s production code, which shrinks from about 730 lines to about 250.

**Added:** the trait and scratch (about 30), the track-side adapter (about 90), the capped row (about 30).

**Kept:**
- The fork hook as is (104 changed lines in two files).
- The `prepare_advance`/`finish_advance` split and `Solution::response` (mobility only).
- `disk_minimum`.
- Every witness: energy, both-body momentum, the exact sliding impulse, slab hold at 1/3/6/12 substeps, the readback bit-check, and the two observation probes.

**Fix regardless:** `DriveTick::contact_only` should keep the real supply and intent and zero only the sampled load (`src/track/transmission/advance.rs`, base-truths N1).

**Cheapest refuting check:** on the lane as it stands, change only the body of `minimum` to the capped rule (about 30 lines) and run the lane's existing targeted tests plus the parked-slab probe. The recommendation is refuted if any of these happens:
- the energy witness fails;
- the slab hold fails;
- the exact sliding impulse fails for a single brake release;
- parked-slab creep exceeds round 1's ≤0.001 m/s.

A second, even cheaper signal: count Newton iterations per point in round 1 on the slab and rubble runs. If more than two is common, the fallback will carry real load and the claim weakens.

## 4. Land round 0 first?

No. Round 0 alone ships a parked tank sliding 4 to 15 cm/s on a slab (MEASURED), and that is its own change's behaviour. The smallest brake fix that is correct is the zero-mobility metric, and making that general across clutch and steering is exactly the recommended row. A brake-only special case would be a rule for one element, written to be replaced. Land once, as round 0's skeleton plus the response metric and capped row.

## 5. What I could not verify

- The descent argument for the capped row is derived on paper, not tested. The corrective second step is the least certain part when two rows change face together.
- Belt inertia (about 4750 kg) and W come from the lens reports, not from asset data; my creep formula rests on them, though it matches four MEASURED rows.
- The cost ratio is a count of block solves, not a timing. Nothing is priced in release on the two-core host, at any tank count, or against `main`.
- How often contact points are fresh in rubble play, which sets how bad alternative b really is.
- The residual 1/n² hull creep at 30° stays unexplained; this formulation does not address it, and the rigid one-point witness suggests it lives in suspension or rotation composition.
- Whether dropping biased-pass friction costs visible travel on fresh contacts in play.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/avian_ext/surface_motion.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/block.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/transmission.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/transmission/advance.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs
