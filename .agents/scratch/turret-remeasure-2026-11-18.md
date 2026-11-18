# #187 remeasured on main 1f59e129: own and remote barrel against the tracer

Subject: the game's turret and tracer presentation. Rounds are game objects; no damage-model state
is read or changed. Every number is MEASURED unless marked DERIVED.

## Method

The 3a capture (`turret-cursor-3a.md`), repeated on main 1f59e129 (the #396 view pacing law),
dev build with `--features dev_ui,bitprobe`. One dedicated server, two hidden-window clients over
real UDP, each receiving through the seeded conditioner at 80 ms ± 10 ms; the uplink carries no
added delay. `OVERMATCH_UNATTENDED=1`, `OVERMATCH_MUTE=1`, the machine lock held for each run.

- **Sweep, 4.6°/s:** `scripts/shot/run-turret-cursor.py fire --seed 1..5`, unchanged.
- **Full traverse, 5.75°/s:** 06c2ffab's aim point, `Point((7.0, 100.0, 800.0))`, a half turn
  away from the observer. It keeps the runner's readiness clicks (`fire_interval: 1`) in place of
  the old 196-tick cadence. The runner no longer scripts this workload, so a scratchpad wrapper
  loads the tracked runner and swaps only its `FIRE` string and fire window in memory. Seeds 1..5.
- **Diagnostic, no conditioner:** the sweep with `CONDITIONER` emptied in the same way. Seeds 1..3.
  It tests one hypothesis and is never compared with the 80/10 numbers.

No game code or tracked tooling changed.

The decomposition runs over the existing traces. Every shot-trace row carries `us`, the wall clock
the three processes share. Per shot, at the frame that first draws the tracer, from the shot's own
rows and the server's per-tick bore:

- **V**, the view delay: `(draw.us − fire.us) − (cursor − fire tick) × T`. It is how long after
  the server simulated the cursor tick the client drew it.
- **lead**, the runner's own lead: drawn yaw − the server's yaw at the cursor.
- **lead_ms**: how far past the cursor, in server time, the server's bore reaches the drawn yaw.
- **ahead** = lead_ms − V. It is how far the drawn turret runs ahead of the server's turret at the
  same wall instant: input sampling, send, the server's tick wait, and the own servo.

The per-frame series and the stream-open instant need two more maps. A line fitted to the server's
rows maps ticks to wall time (residual ≤ 24 ms after dropping 1–6 rows written in late frames).
Each client's frame clock maps to wall time through the frames that wrote a `drawn` row (spread
≤ 9.9 ms).

**Startup** shots are those drawn in the first 12 s of the drawing client's stream; **steady**
shots are the rest. Mid-traverse means the server's bore turned more than 1°/s over the tick
before the fire tick (the runner's definition).

## Results, per run (degrees, ms)

Sweep (16 mid-traverse shots per run; 4 startup, 12 steady on the own client):

| run | own barrel vs tracer, startup / steady | remote barrel vs tracer | own lead, startup / steady | V own, startup / steady | ahead, steady | Δ excess own / remote | release batch |
|---|---|---|---|---|---|---|---|
| 1 | 1.150–1.849 / 0.636–0.673 | 0.078–0.182 | 1.072–1.721 / 0.550–0.576 | 230–254 / 113–118 | 5.9–12.4 | 137 / 192 | 16 |
| 2 | 1.506–2.210 / 0.639–0.693 | 0.072–0.185 | 1.399–2.099 / 0.541–0.587 | 299–333 / 112–119 | 6.9–13.6 | 216 / 184 | 16 |
| 3 | 1.170–1.912 / 0.623–0.656 | 0.071–0.200 | 1.090–1.808 / 0.528–0.548 | 231–254 / 111–117 | 0.3–10.5 | 141 / 187 | 15 |
| 4 | 1.203–1.965 / 0.609–0.688 | 0.070–0.157 | 1.108–1.867 / 0.530–0.595 | 231–256 / 112–117 | 1.8–16.2 | 142 / 186 | 16 |
| 5 | 0.939–1.462 / 0.600–0.643 | 0.074–0.192 | 0.854–1.374 / 0.536–0.552 | 185–296 / 111–116 | 2.8–8.3 | 182 / 224 | 5 |
| all | 0.939–2.210 / 0.600–0.693 | 0.070–0.200 | 0.854–2.099 / 0.528–0.595 | 185–333 / 111–119 | 0.3–16.2 | 137–216 / 184–224 | |

Full traverse (10 mid-traverse shots per run; 4 startup, 6 steady):

| run | own barrel vs tracer, startup / steady | remote barrel vs tracer | own lead, startup / steady | V own, startup / steady | ahead, steady | Δ excess own / remote | release batch |
|---|---|---|---|---|---|---|---|
| 1 | 2.858–3.549 / 2.226–2.275 | 0.100–0.219 | 2.790–2.903 / 2.151–2.169 | 231–249 / 118–122 | 252–263 | 132 / 194 | 16 |
| 2 | 2.957–3.690 / 2.158–2.188 | 0.097–0.213 | 2.862–3.035 / 2.080–2.086 | 251–277 / 111–119 | 242–251 | 164 / 195 | 16 |
| 3 | 2.830–3.546 / 2.163–2.207 | 0.092–0.182 | 2.748–2.892 / 2.086–2.108 | 227–257 / 113–120 | 243–255 | 136 / 188 | 16 |
| 4 | 1.674–2.463 / 1.205–1.237 | 0.095–0.174 | 1.583–1.951 / 1.102–1.122 | 196–263 / 112–116 | 70–88 | 146 / 187 | 6 |
| 5 | 3.565–3.928 / 3.507–4.997 | 0.097–0.162 | 3.175–3.865 / 3.428–4.930, rising | 239–279 / 110–113 | 487–744, rising | 164 / 195 | 16 |

- **Δ excess** is the applied view delay 1.5–9.5 s into the stream, above its level after 14 s,
  from the client's own `client_jitter` lines.
- **Release batch** is the number of fixed ticks the subject ran in the frame that released the
  script, which covers 15.625 ms of own aim each.
- **Remote barrel on the stream:** within −0.006 to +0.007° of the server's yaw at the cursor
  (130 shots).
- **The tracer's first point** is 3.91–3.92 m from the drawn muzzle in both views, as in 3a.
- **3a, for reference (old view clock, 3 runs):** own barrel vs tracer 0.635–2.867, remote
  0.074–0.243, own lead 0.536–2.780.

## Steadiness of the own lead

**At 4.6°/s the lead is steady between runs once each stream is 12 s old: 0.528–0.595° over 60
shots in 5 runs.** It is high only in the stream's first ~11 s: 0.854–2.099° over 20 shots. What
differs between a low shot and a high one is the view delay. Neither a hold nor the click's phase
does:

- **The lead is the view delay.** lead_ms − V = ahead is 0.3–16.2 ms on every steady shot. Startup
  shots carry V = 185–333 ms; steady shots carry 111–119 ms.
- **The excess is held for one statistics window.** In all 26 client streams, the 80/10 ones and
  the diagnostic ones, the first view line showing the fall is written 11.0–12.0 s after the
  stream opened. That is W = 10 s (`assets/net_timing.ron` `view_window_s`) after a sample seen in
  the stream's first 1–2 s, then the slew at v = 0.2. While the delay is high, the own client's
  cursor sits 7.9–13.6 ticks behind the newest completed tick (80/10), so the stream is never
  starved and the excess buys nothing.
- **Holds (80/10):** after the first 12 s, 1–11 per run per client, each at most 9.5 ms, except one
  pair in sweep run 5 (36.6 ms own, 36.3 ms remote; below). The first 12 s of the 20 streams hold
  two: 17 ms at 2 s (sweep run 5, remote) and 6.1 ms at 11–12 s (sweep run 4, own).
- **Click phase:** cursor minus fire tick spans 0.00–0.45 ticks in both groups.

3a runs 1 and 3, which started at 1.2–1.9° and settled at 0.54–0.63°, match this startup window in
shape and size. Their traces are gone, so the match is not proven.

**Mid-run:** one rise in ten runs. In sweep run 5, about 48 s into the own stream, both clients
paused 61–65 ms at the same wall instant. On the own client an 86.5 ms gap between arrivals and a
36.6 ms hold followed, and Δ* rose 45.5 ms for the next 10 s. 3a run 2's mid-run return to
1.7–2.8° ran on the old clock, and its traces are gone. Whether it had the same origin cannot be
shown.

**At 5.75°/s the lead is steady within a run but not between runs:** 1.102–1.122°, 2.080–2.169°
(three runs), and 3.43–4.93° rising. V is the same as at the sweep rate (110–122 ms steady). The
difference is ahead:

- **The release batch.** In 8 of 10 runs the script released inside the stream's first long frame.
  That frame ran a 15–16-tick catch-up batch: 16 × 15.625 ms = 250 ms, the 250 ms cap of Bevy
  0.20.1's `Time<Virtual>` max delta (`bevy_time` `virt.rs:86`). The own client integrates every
  tick of the batch toward the new aim (`harness::buffer_input` in FixedPreUpdate,
  `net/client.rs:598`). The server learns the aim from that frame's single input message
  (`send_inputs` in PostUpdate, `input_arrival.rs:417`; the server keeps the newest arrival,
  `:449`), so its turret starts later by the batch:
  - a 16-tick batch (250 ms) gives ahead 242–263 ms (runs 1–3);
  - a 6-tick batch (94 ms) gives 70–88 ms (run 4).

  The same head start occurs at the sweep rate: ahead at the first shot is 32–70 ms in the
  15–16-tick runs and 12 ms in the 5-tick run. There the server's turret catches up, because 4.6°/s is below the powered traverse; at full
  traverse it never can. DERIVED: this is not harness-only. Any aim change sampled in the first
  frame after a client hitch is integrated over that hitch's catch-up ticks (up to 250 ms) on the
  own client before the server receives it.
- **Run 5's rate.** The own drawn turret traversed at 5.85°/s against the server's 5.75°/s, over
  4-s windows, with the hull still to 0.004°/s. The lead therefore grows by about 0.1°/s until the
  turret arrives.

## DERIVED: the own turret on the stream

Drawn from the stream, the own gun would sit where the remote one does: 0.070–0.219° off its
tracer at these rates, and within 0.007° of the server's yaw at the cursor.

Its answer to the mouse would lag by today's lead in time, V plus the uplink side (the steady
ahead at the sweep rate):

- **steady:** 114.8–128.7 ms on this link. That is the steady lead_ms at the sweep rate, which is
  V of 111–119 ms plus 0.3–16.2 ms.
- **in a stream's first ~11 s:** V of 185–333 ms plus the same, about 185–350 ms.

The full-traverse ahead of 70–744 ms would not be added: it exists only because the own turret
integrates ahead of the server. The servo's own lag behind the aim is the same on either path.

This harness delays only the downlink (80 ± 10 ms), so a one-way uplink delay d adds d. On a
symmetric 80 ms round trip, 40 ms each way, V falls by about 40 ms and the uplink adds about
40 ms. That leaves roughly the same 115–129 ms, in line with C7's 127 ms; this assumes the
buffering margin does not change with the link.

## What this could not establish

- **The startup sample's origin.** Ruled out:
  - the conditioner's read-time delay (`apply_seeded_recv_conditioner` starts each packet's delay
    when the client drains it). The excess persists without the conditioner: 149–383 ms on the
    subject and 206–231 ms on the remote, in 3 diagnostic runs;
  - a hold;
  - the own servo.

  Observed beside it:
  - each stream's first frames include one of 217–367 ms. The excess is not proportional to it:
    the two shortest subject frames (235 and 242 ms, no conditioner) carry the largest excesses
    (346 and 383 ms);
  - the first view line completes 4–26 ticks more than the stream rate, a backlog. Without the
    conditioner this backlog matches the excess within 22 ms in 2 of 3 runs;
  - the server wrote the first client's link-up row 214 ms behind its tick line (a long server
    frame at that connection), and the second's 12 ms behind (sweep run 1 and diagnostic run 3).

  The clock samples x = previous frame − frame-start stamp (`view_clock.rs` `sample`), so a long
  server frame enters x. The per-tick x is not traced, and a reconstruction from the frame rows'
  confirmed tick does not reproduce the sample: in the diagnostic runs it reads ≤ 5 ms. Settling it
  needs a per-sample trace row in `view_clock` (tick, x, departure − frame start, completion
  frame), which is game code.
- **Run 5's 1.7% faster own traverse.** The own client's powered traverse reads replicated
  `EngineRpm`, which `publish_engine_rpm` never writes on a static replica
  (`src/track/sim.rs:656`). That is a candidate only: RPM is not traced.
- **Whether 3a run 2's mid-run rise had the startup cause.**

## Captures

`dist/turret-remeasure/{sweep-1..5,full-1..5,nocond-1..3}/` in bay-1, ignored and wiped when the
bay recycles. The wrapper and decomposition scripts were scratchpad files and are not kept.
