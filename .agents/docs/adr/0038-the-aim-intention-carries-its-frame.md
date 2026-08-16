# 0038 — The aim intention travels in the frame its view is anchored to

Status: ACCEPTED 2026-08-17. Shipped as declared `PROTOCOL_REV = 28`.

Scoped strictly to the TRANSPORT — the frame the intention is measured in on the wire.
[[0001-aim-stored-hull-local]] stands untouched and unsuperseded: the intention is still HELD
hull-locally, and a storage-frame change is still the lever the two stabilization regimes will be
built on.

## Ruling

**`TankCommand::aim` carries an `AimIntent`: a point AND the frame it is measured in.** The
authoring view is the only thing that knows which frame that is, so the frame rides the wire with
the point instead of being a convention a consumer has to guess right.

**The lay follows the aiming view, and the player owns the view.**

| view | anchored to | travels as | what a player holding still gets |
|---|---|---|---|
| third person | the world (the orbit camera does not turn with the hull) | `World` | the lay stays on the place they are looking at, rate-limited by the mount |
| gunner optic, free-aim | the hull (camera, working intent and gun all ride it) | `HullLocal` | the lay is hull-rigid: it sweeps with the tank |

`aim::drive_aim_servos` names whichever arrives in the hull frame of the tick it lays, upstream of
the superelevation lob (which stays a rotation in the hull frame). A servo angle is parent-local, so
that conversion belongs to whoever drives the servo, at the tick it drives it — never to the author.

**A hold is a bearing, not a place, in every view.** `aim::CommittedAim` is hull-local, so the moment
the player stops steering — RMB free-look, an optic with no mouse motion, a pose frame that will not
compose — the intention decays hull-rigid and the gun rides the hull round. Free-look means the view
has STOPPED being the aiming reference: the player is looking away from the gun, so nothing holds it
on anything. That is the no-stabilization ruling, and it is why a world transport for third person
cannot leak into one.

## Why the third-person half had to move

Latency is a rotation. A hull-local point put on the wire from a world-locked view is a bearing off a
body that turns, so every consumer that re-applies it to a LATER hull pose silently rotates the world
intent by ω × (age of the value):

| consumer | age | error at ω = 10–20 °/s |
|---|---|---|
| the client's own turret | ~4 ticks (input delay + the authoring frame), ~63 ms | 0.6–1.3° |
| the authority that actually fires | rtt/2 + interp delay, ~110–160 ms | 0.7–2.2° |

At 300–500 m that is metres between the target the player is holding the crosshair on and the shell
the server echoes back — invisible while parked (ω = 0), absent in single-player (no bridge, no
delay), and therefore never diagnosed from the feel of it. The bias is DIRECTIONAL: it cancels part
of the mount's own lag on one side of a pivot and adds to it on the other, so a one-sided measurement
flatters it (measured: 0.298° turning one way, 2.089° the other, against the mount's own 0.558°).

## Why the optic half had to stay

The gunner optic never had this bug and must not be given one. Its camera is hull-anchored
(`camera::gunner_camera`), its working intent is a yaw/pitch off the hull
(`sight::GunnerIntent::local_dir`), and the gun is bolted to the hull — a hull-local point is
therefore already invariant across the delivery gap, exactly.

Naming it in the world would compose it with the hull the CLIENT is rendering (the interpolated
stream, which lightyear's clamp freezes then steps) and decompose it against the hull the AUTHORITY
has, importing the difference between the two as noise in the FIRED lay, for nothing. Measured on the
same fixture at ω = 10 °/s with six-tick freezes: **0.782° peak-to-peak, against 0.000° hull-local.**
The default optic is scheme A at ~6.9° FOV, so that is over a tenth of the optic's height.

## What this does NOT fix

**The rendered bore still staircases when the interpolated hull does.** The clamp's step carries the
whole tank, and a rate-limited mount can only walk the lay back over the following ticks. Measured at
ω = 10 °/s with six-tick freezes: 0.773° peak-to-peak before this change, 0.790° after — the
transport was never what produced it. That is the interpolated hull's smoothness to answer for, and
no line of this change is entitled to claim it.

What the transport owns is the COMMANDED bearing, and that is now flat in both views.

## The residual, named

Under genuine input starvation lightyear recirculates the last command it received. For a
third-person `World` point that means the authority holds that place for the starvation interval
instead of sweeping with the hull, by ω × (starvation): about 4° at 20 °/s over 200 ms. It is bounded
by the starvation, it costs a frame of unstabilized honesty and never a frame of accuracy, and it
needs no machinery. The optic's `HullLocal` recirculates as a bearing and is not affected at all.

Menus are not this case: blocked input sends `aim: None`, on which `drive_aim_servos` holds the last
parent-local servo target — hull-relative, and honest.

## The regression net

Six laws in `aim`, driving the SHIPPED systems — `commit_aim` and `sight::drive_gunner_aim` author,
`drive_aim_servos` bridges, `tank::drive_servos` integrates the real mechanism at the seed vehicle's
authored rates — across the two hulls the seam actually has (the client's rendered one authors, the
authority's true one lays). Every hull in the net is rotated AND translated, and away from the
origin, so a `transform_vector3` standing in for a `transform_point3` cannot cancel.

| law | superseded transport | now |
|---|---|---|
| one look, two headings: does the pick name one place? | 3466 m apart at 10 km | 0.000 m |
| a place laid against a hull it never saw | misses by the full 20° | 0.000° |
| 10 °/s pivot, crosshair held, BOTH directions | 0.298° / 2.089° | 0.558° / 0.558° = ω²/2a − \|ω\|·dt |
| freeze-then-step, third-person commanded bearing | 0.784° swing | 0.003° (the turret ring's own parallax) |
| optic held through the same freeze-step | 0.782° imported | 0.000° |
| free-look held through a pivot | the gun sweeps with the hull | unchanged, and guarded |

Confirmed red against six mutations: third person authoring hull-local (3 laws), a world-space hold
(the doctrine guard), a world round trip on the optic (the optic law), and `transform_vector3` /
`transform_point3` swapped at each of the three seam sites (5, 3 and 1 laws).

## Wire

`PROTOCOL_REV` 27 → 28. `Option<Vec3>` becomes `Option<AimIntent>`, so the own-type graph moves and
`WIRE_TYPES_HASH` and the manifest fingerprint are re-pinned with it. Two peers on opposite sides lay
the same command's servos at different bearings the moment the hull leaves yaw zero, so this is a sim
skew as much as a format change — exactly what
[[0018-wire-surface-fingerprinted-and-refused]]'s REV exists to refuse. **Deploying this requires
deploying the server**: a REV-27 droplet turns REV-28 clients away at the handshake.
