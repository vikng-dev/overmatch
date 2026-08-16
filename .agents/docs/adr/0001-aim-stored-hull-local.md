# Aim stored hull-local — unstabilized (WW2) is the default

> **Status: accepted; superseded by [[0038-aim-travels-in-world-space]]** (2026-08-16, declared
> `PROTOCOL_REV = 28`): the intention travels as a WORLD point, and the hull-local form is derived
> by whoever drives the servo at the tick it drives it. The gun stays unstabilized — that is a fact
> about the mount's authored slew rate, not about a storage frame. What the storage frame actually
> decided was the meaning of a HELD intention, and holding now holds a spot on the world.

The aim point is stored in the **hull's local frame**, so the gun holds a hull-relative bearing and sweeps as the hull turns — WW2 behaviour, no gun stabilization. We chose this as the baseline because it's the simplest model and it falls out for free from holding the (hull-local) servo targets during free-look.

Modern stabilization then becomes a deliberate gameplay mechanic layered on top, implemented purely as a change of the aim's **storage frame**: a world *ray* gives directional stabilization, a world *point* gives point stabilization (see the `Stabilization` glossary entry).

Recorded because the hull-local default is a deliberate realism choice, not an oversight: a future reader seeing the gun sweep with the hull might otherwise "fix" it.
