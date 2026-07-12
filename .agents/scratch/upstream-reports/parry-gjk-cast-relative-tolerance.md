# parry3d 0.27: GJK shape-cast stagnation exit returns unrefined lower TOI bound — ~140 mm error vs large colliders

**Target:** github.com/dimforge/parry · parry3d 0.27 · **Severity for us:** HIGH (fixed f4a24c2) · **Status:** FILED 2026-07-11 — issue dimforge/parry#429, PR dimforge/parry#430 (branch fix/shape-cast-stagnation-refine on vikng-dev/parry)
**Automatic retirement tripwire:** `tests/spherecast_scale.rs` FAILS when parry fixes this (it
asserts the raw TOI error stays > 10 mm at 500 m) — that failure means: file the workaround for
removal. (Verified 2026-07-10: against the candidate fix the raw error at 500 m drops to
0.00024 mm, so the tripwire fires as designed.)

## Suggested title

Shape-cast TOI error scales with target shape extent: the upper-bound-stagnation exit returns
the unrefined lower bound

## Mechanism (CORRECTED 2026-07-10 — instrumented in parry3d 0.27.0 source; our original
## relative-tolerance attribution was wrong)

`gjk::minkowski_ray_cast` (parry3d-0.27.0 src/query/gjk/gjk.rs:661-780) advances a lower TOI
bound `ltoi` and tracks an upper bound. When the upper bound fails to decrease between
iterations — float cancellation when a large shape's support coordinates (a 1000 m ground
cuboid: ±500 m) are translated into the advanced-origin frame — the "last chance" stagnation
exit fires (gjk.rs:712-715 sets `last_chance` on `max_bound >= old_max_bound`) and returns the
CURRENT LOWER BOUND `ltoi` as the hit (gjk.rs:720-723), still short of true impact by the
stagnated gap. Hit distances come back SHORT (one-sided at 0.27.0), pose-discontinuously
(deterministic per pose, jumping tick to tick).

**What it is NOT (our original report got this wrong):** the relative convergence bound
`eps_rel = sqrt(10 * f32::EPSILON)` plays no role. Instrumented over the 2,400-cast repro
workload: the eps_rel branch (gjk.rs:770-781; gjk.rs:676 only computes `_eps_rel`) fired **0**
times; **2,278/2,278** erroneous TOIs exited via `last_chance`. Structurally, under default
float features that eps_rel branch returns `None` (a miss), never a TOI — it is incapable of
producing the short-hit signature at all.

## Measured

Standalone parry test (avian's exact `cast_shapes` arrangement, sphere r=0.5166 cast at a
cuboid): distance error max 0.25 mm @ 5 m half-extent, 3.6 mm @ 50 m, **139–172 mm @ 500 m**;
in-game per-wheel sampling measured p50 33 mm / p99 134 mm / max 199.75 mm over 19,828 samples.
The witness data (`point1`/`normal1`) is EXACT even when the TOI is wrong. Game-level
consequence before we worked around it: a 551 kN/m suspension spring turned the noise into
10–40 kN/tick force noise — a sustained at-rest hull limit cycle (~12 mm heave, ~2.2 kW pumped)
and a standing amplifier of client/server divergence at contact.

## Suggested upstream fix

Refine the stagnation exit's TOI from the simplex witnesses already in hand (project the
witness separation along the cast direction) instead of returning the raw lower bound; witness
and normal generation stay untouched. A candidate patch exists (branch
`fix/cast-absolute-tolerance`): errors at 5/50/500 m half-extent go 0.246/3.637/139.448 mm →
0.113/0.0023/0.00024 mm, cost confined to the rare stagnation exit (≤4 weighted witness
accumulations). **Disclosure the PR must carry:** (a) the refined TOI is no longer a certified
lower bound — measured overshoot up to +0.113 mm at 5 m half-extent (curvature converts lateral
witness deviation into forward error ≈ r·θ²/2), where 0.27.0 was strictly one-sided short;
(b) a stagnation hit whose refined TOI exceeds `max_time_of_impact` now correctly returns
`None` where 0.27.0 returned a wrongly-short hit.

## Our workaround + removal condition

`sphere_cast_ground_contact` (src/driving.rs, commit f4a24c2): reconstruct distance from the
witness (`point1 + normal1·r` = ball centre), clamped to `[toi, toi + 0.20 m]` (at 0.27.0 the
TOI is never long; post-fix parry can overshoot by ≲0.12 mm, far inside the clamp band, so the
clamp stays valid either way). Conservative fallbacks for penetrating starts and non-finite
witnesses. Remove when the tripwire test fails against an upgraded parry.

## What fixing this unlocks for us

**Clean up.** `sphere_cast_ground_contact` collapses back to `hit.distance` arithmetic: the witness
reconstruction, the `SPHERE_CAST_TOI_SLACK = 0.20` clamp band (whose 0.20 m width is itself a measured
artefact of this defect — sized from 19,828 live samples so it would clip none of them), the
penetrating-start and non-finite-witness fallbacks, and `tests/spherecast_scale.rs` (which exists to
fail on the fix, and is verified to do so: raw error at 500 m half-extent 139 mm → 0.00024 mm against
the candidate patch).

**Optimize.** Marginal in the frame budget (a witness read and a clamp, per wheel per tick). The real
recovery already happened at f4a24c2 — the pre-workaround cast noise fed 10–40 kN/tick of force noise
through a 551 kN/m spring, a sustained at-rest hull limit cycle (~12 mm heave, ~2.2 kW pumped) and a
standing amplifier of client/server divergence at contact. An upstream fix does not re-bank that; it
banks the deletion.

**Explore — the two things the workaround does NOT cover.**

- **Map authoring is currently constrained by this bug.** ADR-0015 carries a defence-in-depth rule:
  *"prefer tiling large static colliders to ≤10 m extents"*, because cast error scales with the target
  collider's extent (0.25 mm at 5 m half-extent vs 139–172 mm at 500 m) — and it notes the rule is *not*
  applied retroactively: *"the 1000 m slab stands until a map-authoring pass."* A fixed parry retires
  that constraint and lets terrain be authored for the terrain's sake.
- **Only the sphere probe is immune, and only because we hand-rolled it.** Every *future* shape-cast
  consumer — track-model contact stations, armor probes, any spatial query against big static geometry
  — inherits the raw defect and would have to re-derive the same witness reconstruction. That is the
  standing tax the fix removes: shape casts become trustworthy by default rather than one function at
  a time.
