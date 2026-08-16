# 0038 — The aim intention travels in a frame latency cannot rotate

Status: ACCEPTED 2026-08-16. Shipped as declared `PROTOCOL_REV = 28` (no byte change: the three
floats are the same three floats, measured in a different frame).

Supersedes [[0001-aim-stored-hull-local]] — the storage-frame half of it, wholly. Applies
[[0037-one-authoritative-timeline-and-view-overlays]]'s overlay discipline to the own turret; the
one-timeline ruling itself is untouched.

## Ruling

**`TankCommand::aim` and `aim::CommittedAim` carry a point in the WORLD.** The player's intention is
a place, and a place cannot go stale.

The hull-local form a servo angle is measured in is not a storage decision at all — it is a
mechanical fact, derived by whoever drives the servo, from the hull pose of the tick it drives it
(`aim::drive_aim_servos`, upstream of the superelevation lob, which stays a rotation in the hull
frame). The gunner optic decomposes the same world point through the live hull rotation for its
yaw/pitch working form, and its resolve round-trips exactly as before.

**The client lays its own gun from the intention that stands now, not from the wire's echo of it**
(`aim::lay_own_aim_from_the_live_intention`). The own turret is the channel 0037 exempts from the
delivery wait; only the authority's servos wait.

## Why

Latency is a rotation. A hull-local point is a bearing off a body that turns, so every consumer that
re-applies it to a LATER hull pose silently rotates the world intent by ω × (age of the value):

| consumer | age | error at ω = 10–20 °/s |
|---|---|---|
| the client's own turret | ~5 ticks (input delay + the authoring frame), ~78 ms | 0.8–1.6° |
| the authority that actually fires | rtt/2 + interp delay, ~110–160 ms | 0.7–2.2° |

At 300–500 m that is 4–19 m of divergence between the green bore dot the player is holding on a
target and the shell the server echoes back — invisible while parked (ω = 0), absent in
single-player (no bridge, no delay), and therefore never diagnosed from the feel of it.

The own-turret half is worse than a bias. The hull it is laid against is the interpolated stream's,
and lightyear's clamp freezes then steps that hull under jitter; a stale intention beats against the
step, so the commanded bearing swings by the whole step and back every period while the player holds
perfectly still. The regression net measures it: 0.938° of square wave, the clamp's step exactly.

## What the player gets that is different

**Free-look (RMB, and the optic's zero-input hold) now holds a spot on the world.** Free-look moves
the camera, not the gun.

This is the whole behavioural surface of the change, because the camera is the aiming device
([[0003-camera-is-the-aiming-device]]) and the orbit camera is world-oriented: while the player is
actively aiming, the screen-centre ray already re-picked a world point every frame and the hull-local
storage never survived a frame. Only a HELD value ever showed the frame it was stored in.

## What 0001 got right, and where it was wrong

Right: the gun is unstabilized, and it stays unstabilized. The mount has exactly its authored slew
rate and never gets a fraction more; nothing counter-rotates the turret against the hull for free;
the bore trails a pivoting hull by the mount's own braking envelope (ω²/2a) and the player feels
every degree of it.

Wrong: that this is a property of the **storage frame**. Stabilization is a MECHANISM — a mount
spending its own authority to cancel hull motion — and its absence is a fact about
`tank::drive_servos`, which knows nothing about any of this. What the storage frame decided was only
what a HELD intention means, and "the gun sweeps while you look around" was never the WW2 fact it
was recorded as; it was the aim point silently rotating, which is the same defect this ADR removes
everywhere else.

## The regression net

`aim`'s three transport laws, over the real bridge and the real mechanism at the seed vehicle's
authored rates. Each was confirmed red against the superseded transport before it was kept:

| law | superseded | now |
|---|---|---|
| authored at hull yaw 0°, consumed at 20° | misses by 20.000° | 0.000° |
| 10 °/s pivot, held crosshair | 1.339° | 0.558° = ω²/2a − ω·dt, the mount's own |
| freeze-then-step clamp, held crosshair | 0.938° swing in the commanded bearing | 0.000° |

## Wire

`PROTOCOL_REV` 27 → 28, and the manifest fingerprint re-pinned with it. Nothing about the bytes
changes — `Option<Vec3>`, same registration, same type graph — so the surface and type hashes do not
move and only a semantic bump can refuse the skew. It is the sharpest case yet for
[[0018-wire-surface-fingerprinted-and-refused]]'s REV: two peers on opposite sides lay the same
command's servos at different bearings the moment the hull leaves yaw zero.
