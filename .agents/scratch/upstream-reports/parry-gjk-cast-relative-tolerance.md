# parry3d 0.27: GJK shape-cast TOI relative tolerance yields ~200 mm error vs large colliders

**Target:** github.com/dimforge/parry · parry3d 0.27 · **Severity for us:** HIGH (fixed f4a24c2) · **Status:** unfiled
**Automatic retirement tripwire:** `tests/spherecast_scale.rs` FAILS when parry fixes this (it
asserts the raw TOI error stays > 10 mm at 500 m) — that failure means: file the workaround for
removal.

## Suggested title

Shape-cast time-of-impact error scales with target shape extent (relative convergence tolerance)

## Mechanism

`gjk::minkowski_ray_cast` (parry3d-0.27.0 src/query/gjk/gjk.rs:661-780) converges on the TOI
with a RELATIVE bound — `max_bound - min_bound <= eps_rel * max_bound` with
`eps_rel = sqrt(10 * f32::EPSILON) ≈ 1.09e-3` (gjk.rs:141-144, 676) — and has an early-return
"upper bounds inconsistencies" path that returns the current lower bound (gjk.rs:713-724).
Against a CSO containing a large shape's support points (a 1000 m ground cuboid: ±500 m), the
relative tolerance is absolute-large: hit distances come back SHORT (one-sided) by up to
~0.2 m, pose-discontinuously (deterministic per pose, jumping tick to tick).

## Measured

Standalone parry test (avian's exact `cast_shapes` arrangement, sphere r=0.5166 cast at a
cuboid): distance error max 0.25 mm @ 5 m half-extent, 3.6 mm @ 50 m, **139–172 mm @ 500 m**;
in-game per-wheel sampling measured p50 33 mm / p99 134 mm / max 199.75 mm over 19,828 samples.
The witness data (`point1`/`normal1`) is EXACT even when the TOI is wrong. Game-level
consequence before we worked around it: a 551 kN/m suspension spring turned the noise into
10–40 kN/tick force noise — a sustained at-rest hull limit cycle (~12 mm heave, ~2.2 kW pumped)
and a standing amplifier of client/server divergence at contact.

## Suggested upstream fix

An absolute (or hybrid) convergence tolerance for the TOI, or documenting the error bound and
recommending witness-based distance reconstruction for large targets.

## Our workaround + removal condition

`sphere_cast_ground_contact` (src/driving.rs, commit f4a24c2): reconstruct distance from the
witness (`point1 + normal1·r` = ball centre), clamped to `[toi, toi + 0.20 m]` (TOI is provably
never long), with conservative fallbacks for penetrating starts and non-finite witnesses. Remove
when the tripwire test fails against an upgraded parry.
