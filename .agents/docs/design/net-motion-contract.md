# Server-stream motion during missing snapshots

Subject: the game's netcode and damage model, game state only.

Hull presentation uses only the server stream. When the interpolation cursor passes its newest
confirmed pose, Lightyear holds that sample until another arrives. `net::frontier` observes this
starvation without writing position, rotation or velocity. Fresh samples resume ordinary
interpolation; no projected trajectory or correction blend survives beside it.

The rejected constant-velocity projection assumed terrain traction bounded hull acceleration.
It omitted recoil and collision impulses, gravity away from supporting contact, contact on
materials that grip harder than the base ground, and angular acceleration. Contact impulse has
no continuous acceleration ceiling in the current motion contract. A traction coefficient and
a visibility tolerance therefore cannot certify a positive projection horizon. Adding another
coefficient would preserve the missing premise. No new tank-specific limit or tuning knob is
introduced to pretend otherwise.

The interpolation buffer covers the full observed receipt spread plus its authority-interval
gap term, with the RTT anchor compensation described by `net::interp_delay`. Receipt time includes
a client frame's socket-drain delay; excluding stalled frames would understate that observed
spread. Samples before play or timeline synchronization remain outside the established stream.
The debug readout reports the same full spread, with display severity expressed in authority
intervals rather than a physical horizon.

Consequences: a clean receipt stream retains its prior interval floor. A variable stream can
require more presentation delay than the rejected discounted law; this lane makes no new feel or
latency measurement and takes no capture. Buffer starvation remains observable and possible
outside the empirical coverage. It is a held server sample, with no claim that the unseen future
remains within a spatial error tolerance.

The regression tests pin unchanged poses during starvation and after a fresh impulse sample,
full spread charged to the delay, and inclusion of stalled receipt samples. A future projection
proposal must first supply a complete motion/impulse and angular-error contract; it cannot reuse
the traction-only premise.
