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

The interpolation buffer pays its empirical receipt-spread term in full, plus its authority-interval
gap term, with the RTT anchor compensation described by `net::interp_delay`. Each packet's
lateness is taken at the last read that lacked it (the frame before the one that drained it), so
a client frame stall on a synthetic unchanged path adds no upper-tail lateness, while a link burst
under ordinary frames keeps its depth less one frame (`net::sync_margin::record_packet_arrival`).
The two stamps include sender pacing and the client's frame schedule. Their mixed-stamp spread is
an empirical presentation term, not an independent measurement of the link's worst lateness.
DERIVED: at the full ring's capacity, `N = 3840` and `p = 1 - 1/N`, the inverse-CDF rank is
`ceil(p*N) = N-1`: the second-largest sample, with one sample outside coverage. The budget permits
that one escape; it does not certify coverage of the maximum. Sample eviction gives nominal
retention of 60 s at 64 samples/s; skipped remote ticks lengthen wall-clock retention. Samples
before play or timeline synchronization remain outside the established stream. The debug readout reports the same full
spread, with display severity expressed in authority intervals rather than a physical horizon.

Previously reported MEASURED `min_delay`, one clean loopback session per row (local server, one
scripted idle client, no conditioner, 2026-11-01), armed value, then after the own tank's arrival stall:

| Build | Headless | Hidden |
|---|---|---|
| Discounted law with the stalled-frame gate (cf4fa4e4) | 17.9 ms; 61.3 ms for 0.6 s, back to 18.0 ms | 22.5 ms; 162.2 ms, two resyncs, back to 18.1 ms |
| Full spread, drain stamp alone (e5d0cdc7) | 26.2 ms; 113.8 ms held for the 60 s ring | 26.5 ms; 1170.0 ms, four resyncs, client panic |
| Full spread, last-read lateness (fef25138) | 22.0 ms; 110.7 ms for 0.5 s, back to 24.9 ms | 24.1 ms; 212.3 ms, three resyncs, back to 32.1 ms in ~5 s |

The headless and hidden columns are different runs; the hidden results include renderer stalls
and timeline resyncs. They are retained as the prior lane's measurements, not a new measurement
or a timing comparison under this lane's concurrent build load.

Consequences: a clean receipt stream pays its measured spread in full where the discounted law
floored it: DERIVED from the first and last rows, their values differ by 1.6–4.1 ms armed and
6.9–14.0 ms after the stall; those runs reported a MEASURED 5–15 ms spread, each session at its own
RTT. A variable stream can require more
presentation delay than the rejected discounted law. Buffer starvation remains observable and
possible outside the empirical coverage. It is a held server sample, with no claim that the unseen
future remains within a spatial error tolerance.

The regression tests pin unchanged poses during starvation and after a fresh impulse sample,
the empirical spread charged to the delay, the synthetic unchanged-path stall adding no
upper-tail lateness, the quantile's one-sample exclusion at capacity, and a link burst retaining
its depth under ordinary frames. A future projection proposal must first supply a complete
motion/impulse and angular-error contract; it cannot reuse the traction-only premise.
