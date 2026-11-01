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
gap term, with the RTT anchor compensation described by `net::interp_delay`. Each packet's
lateness is taken at the last read that lacked it (the frame before the one that drained it), so
a client frame stall reads as no lateness while a link burst under ordinary frames keeps its depth
less one frame (`net::sync_margin::record_packet_arrival`). Samples before play or timeline
synchronization remain outside the established stream. The debug readout reports the same full
spread, with display severity expressed in authority intervals rather than a physical horizon.

MEASURED, `min_delay` on one clean loopback session per row (local server, one scripted idle
client, no conditioner, 2026-11-01), armed value, then after the own tank's arrival stall:

| Build | Headless | Hidden |
|---|---|---|
| Discounted law with the stalled-frame gate (cf4fa4e4) | 17.9 ms; 61.3 ms for 0.6 s, back to 18.0 ms | 22.5 ms; 162.2 ms, two resyncs, back to 18.1 ms |
| Full spread, drain stamp alone (e5d0cdc7) | 26.2 ms; 113.8 ms held for the 60 s ring | 26.5 ms; 1170.0 ms, four resyncs, client panic |
| Full spread, last-read lateness (this record) | 22.0 ms; 110.7 ms for 0.5 s, back to 24.9 ms | 24.1 ms; 212.3 ms, three resyncs, back to 32.1 ms in ~5 s |

Consequences: a clean receipt stream pays its measured spread in full where the discounted law
floored it: the first and last rows differ by 1.6–4.1 ms armed and 6.9–14.0 ms after the stall;
those runs read a 5–15 ms spread, each session at its own RTT. A variable stream can require more
presentation delay than the rejected discounted law. Buffer starvation remains observable and
possible outside the empirical coverage. It is a held server sample, with no claim that the unseen
future remains within a spatial error tolerance.

The regression tests pin unchanged poses during starvation and after a fresh impulse sample,
full spread charged to the delay, a client stall adding nothing to it, and a link burst keeping
its depth. A future projection proposal must first supply a complete motion/impulse and
angular-error contract; it cannot reuse the traction-only premise.
