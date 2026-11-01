# Reflected track inertia from construction (#259)

Subject: the game's driving model, game state only.

The running-gear load gate derives the extra per-side inertia at belt speed:

`I_belt = link_count * link_mass + I_sprocket / r_pitch^2 + I_idler / r_pin^2 + sum(I_station / r_tread^2)`.

Each moment divides by the square of the radius the belt turns its wheel at. Each station moment
covers every rotating disc at its declared axle; a road wheel rolls on the straight belly run's
inner face, which moves at belt speed, so its radius is its tread. The belt wraps the sprocket
and the idler, whose wrapped links turn about the hub with the pin line: the sprocket uses the
chord-exact pitch radius from the declared tooth count and the sim artifact's pin pitch, and the
idler its pin-line radius, the declared contact radius plus the shoe's measured `pin_to_inner`
(`track::derive::pin_line_radius`, the circle the route wraps and the view spins the idler on).
Running-gear admission reads both wrap radii off the rig's rest circles. The hull's total mass already
carries the running gear, so this sum adds only motion relative to the hull. Engine crank,
flywheel and clutch inertia remains separate on the engine side of the coupling.

`TrackSpec::reflected_inertia` owns the sum. The actual running-gear gate checks the common
inertia required by the current joint drive; distinct side totals are refused before play,
naming the construction fields and the need for a drive that integrates separate inertias.
It does not silently approximate an asymmetric pair. Only floating-point construction
roundoff is averaged. Sandbox count/teeth changes refresh both the sum and transmission tables.

## Authored facts and their limits

`powertrain.inertia` is deleted. Complete-link mass and every complete rotating assembly's
axial moment are required sourced facts. The shipped sheet labels its unverified link mass and
new axial moments `Unknown`, naming what would settle them. There is no missing-field default.

The wheel estimates are provisional uniform-disc estimates, not measurements: assumed station
assembly mass at its tread radius; assumed sprocket/idler assembly mass and radius. A real
assembly's radial mass distribution or an axial spin test supersedes those estimates. A view
mesh's decorative vertices do not supply mass distribution. The sheet carries these unknowns;
the generic law carries none of their values.

## Consequence, DERIVED from the current sheet

All values in this section are DERIVED, conditional on those unknown facts. The shipped
vehicle is the comparison fixture, never a pass condition for the generic law.

| Per-side contribution | DERIVED equivalent inertia (kg) |
|---|---:|
| Complete links: declared count times assembly mass | 2,910.00 |
| Declared roadwheel station assemblies | 1,600.00 |
| Declared idler axial moment over pin-line radius squared | 90.25 |
| Declared sprocket axial moment over pitch radius squared | 149.61 |
| Total | 4,749.86 |
| Replaced independent authoring | 16,000.00 |
| Unexplained excess in the old authoring | 11,250.14 |

The new sum is DERIVED 70.31% below the old authoring. It contains every declared wheel family;
there is no omitted declared-wheel term to restore. The unknown complete-link mass and wheel
moments, and undeclared gearbox/final-drive rotating moments, are the missing facts to settle.
The old authoring provides no source for its excess. Keeping the present wheel terms would
require a DERIVED link assembly mass of about 145.98 kg to recover it, against the provisional
30 kg in the sheet. No fact supports that change, so no term is scaled to recover the old number.

| Flat-ground consequence | Old inertia | Construction sum |
|---|---:|---:|
| Hull plus both belt inertias (kg) | 89,000.00 | 66,499.73 |
| Full-service brake deceleration (m/s^2) | 2.15730 | 2.88723 |
| Full-throttle no-slip acceleration in top gear at peak torque (m/s^2) | 0.50377 | 0.66809 |

Braking follows `TrackGear::braking_deceleration`: the material-pair force cap exceeds the
service-brake capacity here, so force is the sheet's two brakes. The DERIVED gain is 33.84%.
For the acceleration row the straight, flat, no-slip operating point has no rolling or external
resistance. Full fuel releases compression drag. The crank remains locked to the belt, so
`a = (torque_at_peak * k) / (hull_mass + 2 * I_belt + J_engine * k^2)`, where
`k = top_gear_reduction / sprocket_pitch_radius`. The DERIVED engine reflection is 2,482.29 kg
and drive force 46,085.92 N; the DERIVED gain is 32.62%. This defines an operating point,
not a claim of constant acceleration throughout launch, clutch slip, shifting or wheelspin.

Doubling complete-link mass at fixed total vehicle mass (a different internal mass allocation)
adds DERIVED 2,910 kg per side. The actual brake predictor then reads DERIVED 2.65488 m/s^2,
against 2.88723 m/s^2 before, exactly the change from the added denominator mass.

## Verification

The class tests isolate every wheel family and link mass, refuse missing/invalid moments and
unrepresentable side totals, and build the real `TrackGear` from a doubled-link-mass sheet to
check the derived braking change. FIXTURE numbers remain under `cfg(test)`.

MEASURED final batch: **532 passed, 0 failed, 8 ignored**, selected by `track spec headless_test
pen_cam field_casts doc_citations ui_ascii net_boundary bake::`. Formatting and locked workspace,
all-target CI clippy passed with `dev_ui,bitprobe`. No full-suite run was made. Builds and tests
used the shared machine lock, `OVERMATCH_CARGO_LOCK_WAIT_S=10800`, unattended and headless settings.
No captures, pricing, agents, issue comments or pushes were performed.

MEASURED headless output: full-throttle top gear held **10.682 m/s**, last-second spread
**0.0020 m/s**; neutral steer **-0.1338 rad/s**, **0.7100** of its DERIVED kinematic ceiling.
Against the earlier review record's MEASURED **10.684 m/s**, top speed shifts about **-0.002 m/s**
(DERIVED difference at the reported precision); neutral steer is unchanged at reported precision.
The acceleration and brake comparisons above remain derivations, not trajectory measurements.

The first batch exposed a false premise in the replay fixture: its hard-coded world height
placed the nominal rolling slope approach in free fall. MEASURED trace: **36 airborne ticks**,
then an airborne belt upshift before landing and immediate hill hold; grade confirmation was
never exercised. The lighter belt exposed that fixture error. Placement derives from the
actual terrain oracle and the running gear's rest envelope along the slope normal, from a seed
whose cast foot lands on the 20° face itself. The seed first chosen cast onto the flat slab short
of the face, so the hull started with its rear stations buried in the slab (law-inertia review,
`sweep-drive-3-review.md` part 9). The fixture now asserts, at the first slope tick and at the tick
hill hold latches, that every loaded contact lies on the face's plane through the foot. Every prior
grade-memory and bit-exact replay assertion remains.
Temporary trace instrumentation is removed. No drive, ground or grip law was tuned to repair it.

Structural spec validation and the spawn crank seed use nominal transmission geometry only;
those tables never drive a belt. Inertia balance and its coupling divisor require the actual
pitch radius and are checked at running-gear admission. A class regression balances unequal
sprocket moments with a roadwheel moment at the actual fixture radius and verifies that a
nominal-radius balance check would refuse that valid construction.

The implementation and tests land together in "driving: derive belt inertia from declared running gear (#259)".
The review leftovers and routed findings are recorded in `sweep-drive-3-review.md`, part 8.
MEASURED final gate clock: 2026-11-01 18:50:57 UTC; the lane began at 18:22:22 UTC.
DERIVED elapsed through that gate: **28 minutes 35 seconds**.
