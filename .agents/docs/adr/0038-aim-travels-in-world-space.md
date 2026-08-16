# 0038 — The aim intention travels in a frame latency cannot rotate

Status: ACCEPTED 2026-08-16. Shipped as declared `PROTOCOL_REV = 28` (no byte change: the three
floats are the same three floats, measured in a different frame).

Scoped strictly to the TRANSPORT. [[0001-aim-stored-hull-local]] stands untouched and unsuperseded:
the intention is still HELD hull-locally, the gun is still unstabilized, and a storage-frame change
is still the lever the two stabilization regimes will be built on.

## Ruling

**`TankCommand::aim` carries a point in the WORLD; `aim::CommittedAim` keeps holding a point in the
HULL.** The intention decays where a WW2 lay decays and travels where nothing can rotate it.

Every frame, the active commit system names the held bearing as the world point it stands on under
the hull pose that stands NOW (`aim::CommittedAim::in_world`, `sight::drive_gunner_aim`'s publish)
and authors THAT. The value on the wire is therefore never older than the frame it was authored in,
so no delay downstream of the author — the input delay, the link, the interpolation delay — has an
angle to rotate it through. `aim::drive_aim_servos` drops it back into the hull frame of the tick it
lays, upstream of the superelevation lob, which stays a rotation in the hull frame.

The hull-local form a servo angle is measured in was never a storage decision: it is a mechanical
fact, derived by whoever drives the servo from the hull pose of the tick it drives it.

## Why

Latency is a rotation. A hull-local point put on the wire is a bearing off a body that turns, so
every consumer that re-applies it to a LATER hull pose silently rotates the world intent by
ω × (age of the value):

| consumer | age | error at ω = 10–20 °/s |
|---|---|---|
| the client's own turret | ~5 ticks (input delay + the authoring frame), ~78 ms | 0.8–1.6° |
| the authority that actually fires | rtt/2 + interp delay, ~110–160 ms | 0.7–2.2° |

At 300–500 m that is 4–19 m between the target the player is holding the crosshair on and the shell
the server echoes back — invisible while parked (ω = 0), absent in single-player (no bridge, no
delay), and therefore never diagnosed from the feel of it.

## What a hold still means

**Free-look (RMB, and the optic's zero-input hold) holds a BEARING, and the gun rides the hull
round.** Stop picking while the hull pivots and the lay sweeps off target at the hull's own rate —
0001's unstabilized WW2 lay, unchanged and now guarded by a test that fails against a world-space
hold. Nothing here gives the mount authority it did not have: it spends its authored slew rate and
loses to a pivot exactly the ground that rate cannot make up.

## What this does NOT fix

**The rendered bore still staircases when the interpolated hull does.** Under jitter lightyear's
clamp freezes the hull, then steps it; the step carries the whole tank, and a rate-limited mount can
only walk the lay back over the following ticks. Measured on the seed vehicle at ω = 10 °/s with
six-tick freezes: 0.773° peak-to-peak before this change, 0.790° after — the transport was never
what produced it. That is the interpolated hull's smoothness to answer for, and no line of this
change is entitled to claim it.

What the transport does own is the COMMANDED bearing, and that is now flat: 0.938° of square wave
before, 0.000° after.

## The residual, named

Under genuine input starvation lightyear recirculates the last command it received, and the last
command is now a world point — so the authority holds that spot for the starvation interval instead
of sweeping with the hull, by ω × (starvation): about 4° at 20 °/s over 200 ms. It is bounded by the
starvation, it costs a frame of unstabilized honesty and never a frame of accuracy, and it needs no
machinery. Menus are not this case: blocked input sends `aim: None`, on which `drive_aim_servos`
holds the last parent-local servo target — hull-relative, and honest.

The free-look sweep also trails the hull by the delivery gap, ω × delivery (0.781° at 10 °/s), since
the point that arrives names the bearing as it stood when it was authored. That is the trade the
ruling makes deliberately: an intention's age shows as phase on a sweep the player is not sighting
along, instead of as a rotation on the target they are.

## The regression net

`aim`'s four laws, over the real bridge and the real mechanism at the seed vehicle's authored rates.
Each was confirmed red against the superseded transport before it was kept:

| law | superseded | now |
|---|---|---|
| authored at hull yaw 0°, consumed at 20° | misses by 20.000° | 0.000° |
| 10 °/s pivot, crosshair held on a target | 1.339° | 0.558° = ω²/2a − ω·dt, the mount's own |
| freeze-then-step clamp, commanded bearing | 0.938° swing | 0.000° |
| free-look held through a 10 °/s pivot | the gun sweeps with the hull | unchanged, and guarded |

## Wire

`PROTOCOL_REV` 27 → 28, and the manifest fingerprint re-pinned with it. Nothing about the bytes
changes — `Option<Vec3>`, same registration, same type graph — so the surface and type hashes do not
move and only a semantic bump can refuse the skew. It is the sharpest case yet for
[[0018-wire-surface-fingerprinted-and-refused]]'s REV: two peers on opposite sides lay the same
command's servos at different bearings the moment the hull leaves yaw zero.
