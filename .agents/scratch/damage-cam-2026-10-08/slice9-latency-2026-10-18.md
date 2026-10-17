# Slice 9a — pen cam delivery latency, MEASURED (2026-10-18)

Instrument commit `e3969f4` on `worktree-agent-a3d09e79d71d46c0e`. Frame cost (slice 9b) is
not here: it needs a visible window.

## Verdict

- **No explanation timed out.** 284 stored across four runs, 0 `TimedOut`, 0 sent-but-lost.
  Slowest release 408 ms against `IMPACT_TIMEOUT_S` = 1000 ms.
- **The explanation always beats the drawn impact.** 0 of 284 arrived already `Presented`. Each
  one waited 7–288 ms for the client to draw its impact. The cam is gated by the impact draw, not
  by the explanation.
- **Wire latency is the link and nothing else.** The authority sends the record 0.3 ms (p50)
  after it resolves the impact. Transit is the conditioned one-way latency plus under a frame.
- **`open` grows with the interpolation delay**, and that delay differs per client (asymmetry
  below).

## Method

- **Instrument.** Behind `SPIKE_SHOT_TRACE` only. No wire or sim change; PROTOCOL_REV untouched.
  - Server: `kf`/`cf` rows (existing) mark an authority outcome resolved. `xsend` marks a
    projection entering transport.
  - Client: `xrx` marks an explanation stored, with its presence on arrival. `xpres` marks its
    release (`presented` / `timed_out`). `xdraw` marks the client drawing a shot's authority
    outcome.
  - Every row carries `us`, the wall clock in µs. All three processes run on one machine, so
    every interval is a difference of `us`.
  - `scripts/shot/pencam_latency.py` joins the rows by shot id + outcome ordinal.
- **Metrics.**
  - **wire** = server impact → `xrx`.
  - **draw** = `xrx` → own `xdraw`. Negative would mean the impact was drawn first.
  - **open** = server impact → release to a view: `xrx` if presented on arrival, else `xpres`.
  - Ticks = ms / 15.625 (DERIVED, 64 Hz).
- **Workload.**
  - A dedicated server and two scripted clients parked 8 m apart on the flat patch
    (`SPIKE_SPAWN_POSE`).
  - Each client clicks its 88 every 256 ticks (`SPIKE_FIRE_PRIMARY`, new) and holds its MGs
    throughout.
  - Client a aims at b's engine bay; b aims at a's front hull.
  - Each run lasts 200 s (12 800 ticks).
  - `LAT`/`JIT` condition each client's receive path (server → client, the direction an
    explanation travels). Jitter is seeded (`SPIKE_JITTER_SEED`).
- **Launch rules.** Every process ran with `OVERMATCH_MUTE=1`. The clients ran under
  `SPIKE_SIM_WINDOWED=1` without `SPIKE_SIM_VISIBLE`: hidden window, macOS activation revoked
  (`net/client.rs` ~264). `OVERMATCH_CAPTURE` itself is ignored in scripted mode.
- **Validity.** 2 connects per run, so no lane was burned.

## Main gun — per condition and role

ms (ticks). Each recorded shot gives one shooter copy and one victim copy.

| link | role | n | wire p50 / p90 / max | draw p50 / p90 / max | open p50 / p90 / max | TimedOut |
|---|---|---|---|---|---|---|
| 0 / 0 | shooter | 37 | 8 (0.5) / 15 (1.0) / 17 (1.1) | 41 (2.6) / 57 (3.7) / 105 (6.7) | 52 (3.3) / 69 (4.4) / 116 (7.4) | 0 % |
| 0 / 0 | victim | 37 | 7 (0.4) / 11 (0.7) / 13 (0.8) | 40 (2.6) / 53 (3.4) / 61 (3.9) | 46 (3.0) / 64 (4.1) / 69 (4.4) | 0 % |
| 60 / 10 | shooter | 35 | 77 (4.9) / 86 (5.5) / 93 (5.9) | 42 (2.7) / 57 (3.6) / 82 (5.2) | 120 (7.7) / 139 (8.9) / 144 (9.2) | 0 % |
| 60 / 10 | victim | 35 | 72 (4.6) / 79 (5.1) / 81 (5.2) | 46 (3.0) / 65 (4.2) / 76 (4.9) | 115 (7.4) / 137 (8.8) / 141 (9.0) | 0 % |
| 120 / 30 seed 1 | shooter | 36 | 140 (8.9) / 160 (10.2) / 167 (10.7) | 167 (10.7) / 203 (13.0) / 234 (14.9) | 315 (20.2) / 331 (21.2) / 350 (22.4) | 0 % |
| 120 / 30 seed 1 | victim | 36 | 130 (8.3) / 155 (9.9) / 177 (11.3) | 62 (4.0) / 125 (8.0) / 237 (15.2) | 193 (12.3) / 257 (16.5) / 350 (22.4) | 0 % |
| 120 / 30 seed 2 | shooter | 34 | 144 (9.2) / 170 (10.9) / 212 (13.6) | 151 (9.7) / 274 (17.5) / 288 (18.5) | 316 (20.2) / 393 (25.2) / 408 (26.1) | 0 % |
| 120 / 30 seed 2 | victim | 34 | 133 (8.5) / 151 (9.7) / 164 (10.5) | 102 (6.5) / 204 (13.1) / 264 (16.9) | 231 (14.8) / 342 (21.9) / 397 (25.4) | 0 % |

Server impact → send: p50 0.3 ms, max 0.73 ms, in every condition. At 8 m the shell ends inside
the tank on its impact tick, so the record finishes on that tick. A perforation that travels on
before it clears the tank would hold longer: DERIVED, not measured here.

Each victim row at 60/10 and at 120/30 includes one `KilledVictim` explanation (a knockout).
That makes 2 kill cam records measured; both were presented.

### MG

The MG produces **no explanation, by design**: `recorder.rs` records `FireMechanism::Single`
only. Here it is load: ~3 720 MG rounds per run, ≈18.6 a second (DERIVED: 3 720 / 200 s), on the
same link through every main-gun sample above.

## The per-client asymmetry at 120/30

Each role row above is mostly one client:

- Client b (connected second) holds 32 of 37 shooter copies; client a holds 32 of 37 victim
  copies.
- Tank a's main gun goes silent after its first ~5 rounds (crew-gated reload; tank b's front-hull
  hits). That cause is not verified beyond the fire log.

Split by client, the role difference disappears and a client difference remains:

| 120 / 30 | client | role | n | draw p50 | open p50 / p90 |
|---|---|---|---|---|---|
| seed 1 | a | victim | 31 | 58 (3.7) | 192 (12.3) / 206 (13.2) |
| seed 1 | a | shooter | 5 | 66 (4.2) | 203 (13.0) / 245 (15.7) |
| seed 1 | b | shooter | 31 | 174 (11.1) | 319 (20.4) / 331 (21.2) |
| seed 1 | b | victim | 5 | 197 (12.6) | 324 (20.7) / 348 (22.3) |
| seed 2 | a | victim | 29 | 94 (6.0) | 230 (14.7) / 246 (15.8) |
| seed 2 | a | shooter | 5 | 80 (5.1) | 243 (15.6) / 257 (16.4) |
| seed 2 | b | shooter | 29 | 231 (14.8) | 383 (24.5) / 394 (25.2) |
| seed 2 | b | victim | 5 | 234 (15.0) | 378 (24.2) / 391 (25.4) |

On the same link, client b draws impacts 110–140 ms later than client a, in both seeds.

The trace shows the mechanism. A client presents the shell when its interpolation cursor reaches
the fire tick. When the fire fact arrives, client a's cursor is ~2–4 ticks behind that tick and
client b's ~10 (`fire_rx.itick` vs `ft`). At 0/0 and 60/10 the two clients agree within a few
ms.

The cause is **not diagnosed**. Client b differs in three ways at once: it connected second, its
main gun fires all run, and it has the other aim. Separating them is the next capture, if the
120 ms tail matters.

## Not measured, and why

- **Frame cost.** It needs a visible window (slice 9b).
- **The TimedOut path.** It never fired (0 of 284), so its curve is empty. Exercising it needs
  loss (`SPIKE_BURST_*` or a lossy conditioner) or a dropped outcome; these runs had no loss.
- **Outcome-less explanations** (`o` null: a round reaching a tank past its first crossing).
  None occurred: two tanks, no second-tank perforation.
- **Range.** Every shot is at 8 m (fixture). Hold-to-send at long range or through a
  perforation is DERIVED above, not measured.
- **Uplink conditioning.** `LAT`/`JIT` condition receive only (server → client), the
  explanation's own direction. The client → server direction is unconditioned.
- **WiFi.** Nothing controlled for a concurrent WiFi scan. The tails stay inside
  latency + jitter + a frame or two (largest wire sample at 120/30: 212 ms), so no transient
  spike carries a verdict.

## Reproduce

From the worktree root (the glbs are LFS: run `git lfs pull` once in a fresh worktree):

```sh
LAT=0   JIT=0  scripts/shot/run-pencam-latency.sh
LAT=60  JIT=10 scripts/shot/run-pencam-latency.sh
LAT=120 JIT=30 scripts/shot/run-pencam-latency.sh
LAT=120 JIT=30 SEED=2 scripts/shot/run-pencam-latency.sh
# each: OUT=/tmp/overmatch-pencam-latency-<LAT>-<JIT> by default, RUN_S=200; it builds, runs the
# server and two hidden muted clients, then prints and writes summary.json. Re-analyze with
uv run scripts/shot/pencam_latency.py --server $OUT/server.server.jsonl \
  --client $OUT/a.client.jsonl --client $OUT/b.client.jsonl --json $OUT/summary.json
```

The first run after a rebuild warms the Metal shader cache. These four runs followed a 40 s and a
120 s warm run.
