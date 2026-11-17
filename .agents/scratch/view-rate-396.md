# View clock pacing: issue 396

Subject: Overmatch's multiplayer tank game, netcode presentation timing; game state only. No damage
model is touched. Lane `lane/view-rate`, merged with `main` at `3fb3c393`.

## What changed

The cursor no longer follows the inverse of the stamp map segment by segment. That rule turned
each server tick's production interval into playback speed: a short interval asked for fast
playback and rode the `(1+v)` clamp, a long one slowed at once. It is deleted, together with the
`.max(previous)` that silently enforced monotonic time.

The cursor advances at the stream's measured rate `b`, steered by a critically damped phase loop
toward the same inverse-map target `S^-1(now - Delta)` (`ViewClock::pace`,
`src/net/view_clock.rs`):

- `dc/dt = b + r`, `dr/dt = -omega^2 z - 2 omega r`, where `z` is cursor minus target in simulation
  seconds and `r` the rate deviation from `b` (`ViewClock::rate`).
- `b` is the window's rate (`ViewClock::window_rate`): T times the ticks the retained samples
  span over the stamp time they span, capped at `1+v`. Lost ticks count. While an outage empties
  the window, `b` holds with the applied delay (`ViewClock::stream_rate`).
- Exact over any frame length: each frame takes the target as moving at `b` and applies the
  closed-form critically damped step, so a 1 s client stall is as stable as a 7.8 ms frame.
- Limits: the frame's advance stays in `[0, (1+v) dt]` and `b + r` in `[0, 1+v]`. Monotonic time
  follows from the nonnegative clamp and from retained facts: the newest completed tick never
  retires below the cursor. The cursor never passes the newest completed tick.
- A frame is held when that tick stops the cursor by more than a U0F16 cursor quantum: the
  request lies past it, or the inverse clamped the target to it. A hold sets `r = -b`, so release
  accelerates from rest. A backlog above `max(view_cut_s, admission_s)` still cuts to the target
  with `r = 0`. Waiting before the first stamp pair and epoch resets are unchanged.
- The delay estimator `Delta* = median interval + max_W(x)` and its slew of v per Real second are
  unchanged. The slew turns a step in `Delta*` into a slope change of v; the loop turns that slope
  change into a smooth rate change.
- `assets/net_timing.ron` is refused at load unless `view_rate < 1`. A delay rising at real time
  would stop presentation time, and `v < 1` bounds `omega` by `e / T`, so every pacing product is
  finite.

### The constant, derived

`omega = e v / T` (`bandwidth`), DERIVED from the owner's `view_rate` Taste v = 0.2 and the
authority tick T = 15.625 ms: omega = 34.79 per second, 1/omega = 28.7 ms. It is a `Resolution`
number with this stability reason:

- From zero rate deviation, the critically damped response to a target phase step never
  overshoots, so it does not spend the buffer the delay target sized. A stored positive rate
  deviation can still carry the cursor past a target it starts on (for `r = v`, by 2.1 ms).
- The rate response to target phase has impulse response `omega^2 (1 - omega t) e^(-omega t)`,
  L1 norm `2 omega / e`, integral zero. Any target phase of peak-to-peak spread `p` moves the rate
  by at most `(omega / e) p = v p / T`. `omega = e v / T` is the widest band for which production
  phase noise within one authority tick never reaches the slew limit; larger phase steps (released
  backlogs) saturate at v. `e` is the law's form constant.

Consequences, DERIVED from the law:

- A slewing delay changes the rate by at most `v omega / e = v^2 / T` = 2.56 per second; the rate
  reaches the full v over about 115 ms instead of in one frame.
- A released backlog B closes to within `v / omega = T / e` = 5.75 ms of the target in `B / v`:
  the rate saturates at v until `2 v / omega` remains, and the critically damped exit leaves
  `4 e^-2 v / omega` at that time.
- A regular server at speed s reads `b = s` once the window holds only that speed, so the cursor
  is the target's phase through a positive kernel of unit area: no steady lead, and no hold while
  the delay target covers the stream.
- A rate error `delta` between `b` and the target leaves at most `2 |delta| / omega` of lead or lag
  in the continuous limit (the kernel `(1 + omega t) e^(-omega t)` is positive with area
  `2 / omega`). At a 10 % error that is 5.75 ms; the unclamped sampled update at 120 Hz gives
  5.33 ms. Such an error lasts only until the window turns over after a speed change.

### Why the window's rate, not `T / median interval`

The window's rate telescopes production phase out: a schedule whose phase stays within p of a
line reads that line's rate to within p over the window's span. The nearest-rank median interval
does not: under alternating production it is the short interval or the long one depending on the
window's parity, so `T / median` flips between them as the window slides. A feedforward reaches
the rate unfiltered, so that flip replays tick timing directly, which is the bug this lane
removes. Measured on the fixtures below, with release at rest:

| Fixture | Feed | min | p1 | p5 | p50 | p99 | outside 1 % | at 1+v | held / episodes |
|---|---|---|---|---|---|---|---|---|---|
| alternating, lateness slack | median | 0.0000 | 0.3003 | 0.6330 | 1.0495 | 1.2000 | 99.6 % | 3.7 % | 71 / 55 |
| alternating, lateness slack | window | 0.2560 | 0.9769 | 0.9927 | 0.9987 | 1.1171 | 12.9 % | 0 | 15 / 15 |
| alternating, constant link | median | 0.3116 | 0.3116 | 0.3116 | 1.2000 | 1.2000 | 100 % | 60.0 % | 480 / 480 |
| alternating, constant link | window | 0.2679 | 0.2679 | 0.2679 | 1.0897 | 1.1762 | 100 % | 0 | 480 / 480 |

Elsewhere the two feeds measured alike: no hold at any regular speed from 0.5 to 1.1, equal
jittered and stall rows. The median turns over a little sooner after a slowdown, 216 against 231
held frames after a step from 1.0 to 0.5. The cost of the window's rate: a stall beyond the
admission cap rebases the schedule by R, which biases `b` by about R / W for one window and steps
back when the stall leaves it.

## Slow servers

Regular production at speed s, constant 40 ms link, 120 Hz client, measured after two windows of
settling. MEASURED held frames and hold episodes per 20 s (2400 frames): main's per-segment rule,
the committed lane before this round (`b = 1`, nominal feedforward), and this lane.

| Speed | main | nominal feed | this lane |
|---|---|---|---|
| 0.5 | 0 | 640 / 480 | 0 |
| 0.6 | 0 | 480 / 384 | 0 |
| 0.7 | 0 | 304 / 304 | 0 |
| 0.75 | 0 | 240 / 240 | 0 |
| 0.8 | 0 | 224 / 224 | 0 |
| 0.9 | 0 | 96 / 96 | 0 |
| 1.0 | 0 | 0 | 0 |
| 1.1 | 0 | 0, 5.331 ms behind | 0 |

The review was right: with nominal feedforward the stops began well above the 73 % this report
used to derive. Completed headroom falls toward zero before each completion, so any steady lead
reaches the ceiling. Under this lane, over 60 s at each of 0.5, 0.6, 0.7, 0.8, 0.9, 1.0 and 1.1:
every frame's rate equals s to four decimals, and the lead stays within -0.0010 to +0.0007 ms,
inside its DERIVED bound of the target's phase range plus quantization (0.0017 to 0.0031 ms).

### After a speed change

MEASURED over the 20 s after a step at two windows, held frames / hold episodes:

| Step | main | nominal feed | this lane | this lane: last hold after the step | lead or lag during the fill |
|---|---|---|---|---|---|
| 1.0 to 0.5 | 430 / 215 | 639 / 479 | 231 / 192 | 9.883 s | -10.354 to +4.481 ms |
| 1.0 to 0.9 | 76 / 76 | 121 / 121 | 51 / 51 | 10.092 s | -10.164 to +4.663 ms |
| 0.5 to 1.0 | 0 | 1 / 1 | 0 | none | lag to 26.625 ms; rate to 1.1553; largest frame change 0.0527 |

A slowdown holds until the window turns over, under every rule: the median interval lags the new
production interval, so the delay target does not cover it, and here the window's rate lags too.
DERIVED bound: the last old-speed sample leaves the window W after its arrival, a link time and a
frame after the change. With the loop's settling (20 / omega) that is 10.623 s, and the test
asserts no hold after it. The phase loop carries the rate error meanwhile, within
`2 |delta| / omega` (28.7 ms for a 0.5 step); the 26.6 ms lag on the speed-up is that bound
nearly reached. A speed-up never holds. This lane holds less than main through a slowdown's fill.
Measuring lateness from an ideal production schedule (below) would cover a slowing server's ticks
at once.

## Release from a hold

Both candidates, MEASURED with the window feed: at rest (`r = -b`, kept) and at the feedforward
rate (`r = 0`, deleted). Held frames / hold episodes, the largest frame-to-frame rate change, and
the first frame's rate after release.

| Fixture | At rest | At the feedforward rate |
|---|---|---|
| 200 ms server stall | 24 / 1; 0.7500; 0.2601 | 24 / 1; 1.0084; 1.0084 |
| 100 ms delay step up, 3 s | 17 / 6; 0.7545; 0.2517 to 0.2821 | 34 / 23; 1.0304; 1.0000 to 1.0304 |
| speed 1.0 to 0.5 | 231 / 192; 0.7903; 0.1275 to 0.2647 | 496 / 287; 1.0107; 0.5000 to 1.0107 |
| speed 1.0 to 0.9 | 51 / 51; 0.7541; 0.2265 to 0.2545 | 289 / 280; 1.0005; 0.9000 to 1.0025 |
| alternating, constant link | 480 / 480; 0.5233; 0.2679 | 1440 / 1440; 0.3482; 1.0095 to 1.0165 |
| jittered, constant link | 144 / 144; 0.7670; 0.2428 to 0.2567 | 224 / 224; 0.3181; 0.9910 to 1.0052 |
| alternating, lateness slack | 15 / 15; 0.6887; 0.2560 to 0.2569 | 15 / 15; 0.1853; 1.0043 to 1.0052 |
| jittered, lateness slack at any phase | 5 / 5; 0.7360; 0.2483 to 0.2525 | 5 / 5; 0.0948; 0.9966 to 1.0009 |

Release at rest holds less or equally everywhere, and only it avoids repeated contact. At the
feedforward rate the cursor re-reaches the ceiling it just left, 2 to 3 times as often. The rest
start costs a deeper one-frame dip, a first frame near 0.25 and larger single-frame changes in the
coverage rows. The stall rows differ only in the restart: 0.26 eased, or a jump to 1.01.

## Reproduction and rate distributions

Fixture (`src/net/view_clock/pacing.rs`): the shipped installer, fed completed stamps through the
replication completion path; 120 Hz client frames; 40 ms constant one-way completion delay;
production phase spread a quarter tick (3.906 ms) peak to peak, either alternating on schedule and
late (intervals 0.75 T and 1.25 T) or a per-tick uniform draw. 60 s measured after two windows of
settling. Per-frame rate is cursor advance in simulation seconds over the frame's Real duration.
All values MEASURED. "Before" is `main`'s per-segment rule; "after" is this lane. Held frames use
the hold definition above. The committed nominal-feed loop's figures under it were 480, 144, 15
and 5 (earlier reported as 480, 102, 8 and 4 under a flag that missed stops ahead of the target).

Constant link (holds included in the distribution):

| Production | Code | min | p1 | p5 | p50 | p95 | p99 | max | at 1+v | outside 1 % | held frames |
|---|---|---|---|---|---|---|---|---|---|---|---|
| regular | both | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 0 | 0 | 0 |
| alternating | before | 0.3417 | 0.3417 | 0.3417 | 1.0750 | 1.2000 | 1.2000 | 1.2000 | 46.7 % | 93.3 % | 1440 / 7200 |
| alternating | after | 0.2679 | 0.2679 | 0.2679 | 1.0897 | 1.1762 | 1.1762 | 1.1762 | 0 | 100 % | 480 / 7200 |
| jittered | before | 0.6114 | 0.8206 | 0.8658 | 0.9963 | 1.1555 | 1.2000 | 1.2000 | 2.2 % | 88.4 % | 294 / 7200 |
| jittered | after | 0.2428 | 0.2516 | 0.8573 | 1.0083 | 1.1314 | 1.1496 | 1.1748 | 0 | 62.3 % | 144 / 7200 |

At a constant link both rules hold: the estimator does not cover production phase (next section).
The distributions there are dominated by holds and their recoveries, so they do not isolate pacing.

Lateness slack (every 61st tick completes a tick late, so the window's lateness maximum leaves
every other tick a tick of slack); the asserted test uses jittered production with the late
ticks on schedule:

| Production | Code | min | p1 | p5 | p50 | p95 | p99 | max | at 1+v | outside 1 % | held frames |
|---|---|---|---|---|---|---|---|---|---|---|---|
| jittered, late ticks on schedule (asserted) | before | 0.8031 | 0.8264 | 0.8636 | 0.9943 | 1.1640 | 1.2000 | 1.2000 | 2.6 % | 88.9 % | 0 |
| jittered, late ticks on schedule (asserted) | after | 0.9654 | 0.9772 | 0.9820 | 0.9997 | 1.0183 | 1.0246 | 1.0405 | 0 | 40.0 % | 0 |
| jittered, late ticks at any phase | before | 0.8058 | 0.8277 | 0.8649 | 0.9947 | 1.1605 | 1.2000 | 1.2000 | 2.4 % | 88.8 % | 4 |
| jittered, late ticks at any phase | after | 0.2483 | 0.9768 | 0.9824 | 0.9999 | 1.0192 | 1.0311 | 1.1561 | 0 | 40.4 % | 5 |
| alternating | before | 0.6400 | 0.8000 | 0.8000 | 0.9833 | 1.2000 | 1.2000 | 1.2000 | 20.1 % | 99.6 % | 16 |
| alternating | after | 0.2560 | 0.9769 | 0.9927 | 0.9987 | 1.0118 | 1.1171 | 1.1573 | 0 | 12.9 % | 15 |

The asserted band for the first row is DERIVED independently of the implementation as
`1 +/- v (spread + applied-delay range / T) + 4 spread / W + quantization` = `1 +/- 0.0540`
(applied delay range 0.190 ms). The `4 spread / W` term bounds the window rate's own movement. The
extremes of the other "after" rows come from holds and the release from rest that follows them.

## Not anticipated by the design

**The delay target covers lateness, not production phase.** A smooth cursor presents the mean
production line, so it needs `Delta >= T + max(d + x) - mean(d)` for production phase `d` and
lateness `x`; the per-segment rule needed the longest interval instead of the median. The target
`median + max_W(x)` leaves the tick that sets the lateness maximum short by its production phase
above the mean, and at a constant link every tick is such a tick. Under alternating production the
nearest-rank median is the short interval about half the time (the delay target swung
51.299–59.114 ms, the full `2 x 0.25 T`), so holds remain even with lateness slack. The pacing loop
needs less margin than the old rule (`mean` instead of `longest`), which is why held frames fell
from 1440 to 480 and from 294 to 144, but it cannot remove them. The same gap holds a slowing
server until the window turns over. The brief keeps the estimator; changing it is an owner
decision. The review's recommendation is the smaller change: measure lateness from an ideal
production schedule, so production lateness enters the existing maximum and the form "median
interval + maximum lateness over W" stays without a percentile. It needs an explicit phase
reference and a rebasing policy for stalls, so accumulated drift does not become permanent delay.
The alternative, a margin of `T` plus the production phase spread, changes the owner-decided
median form.

Real links have jitter, which leaves typical ticks slack below the lateness maximum, so holds there
need a late tick and a late production phase together; the issue does not report holds. The
hosted capture in the issue's done-when will show whether they matter.

## Stall and spike re-measurements

`src/net/view_clock/udp.rs`, real loopback UDP, production protocol and runner schedule, 80 ms RTT
with integer 10 ms downlink jitter, Fixture 12 ms ordinary work, 128 Hz reads. MEASURED; the
"before" column is the per-segment rule on the same fixture, matching
[the slice 2 report](view-clock-slice-2.md).

| Scenario | Held | Cuts | Delay (baseline to peak) | Above baseline | Slow s (before) | Fast s (before) |
|---|---|---|---|---|---|---|
| 200 ms work stall | 179.6875 ms | 0 | 70.3125 to 250 ms | 10.8984375 s, until 11.1484375 s | 0.5625 (0.4453125) | 1.5 (1.34375) |
| 1 s capped work stall | 976.5625 ms | 0 | 70.3125 to 1046.875 ms | 14.8828125 s, until 15.9296875 s | 4.3984375 (4.2734375) | 5.6328125 (5.484375) |
| 500 ms downlink blackout | 484.375 ms | 1, at 0.5 s, 31.4 ticks | 70.3125 to 554.6875 ms | 12.421875 s, until 12.9140625 s | 2.546875 (2.4140625) | 2.546875 (2.421875) |

Holds, cuts and delay match the per-segment rule. Slow and fast mean a nonzero frame rate below
0.99 or above 1.01; the loop's smooth onset and landing put about 0.1 to 0.15 s more frames in
those classes per scenario. The settled 80/10 link, the 60 ms burst lever and the loaded-server
distribution still show zero holds and cuts; the applied delay on 80/10 is 70.3125 ms, inside its
DERIVED band.

Headless installer fixtures, MEASURED:

| Scenario | After | Before |
|---|---|---|
| 200 ms server silence, then admitted debt | held 200 ms in one episode; peak backlog 198.987 ms; within 5.75 ms of the target 975 ms after the peak; DERIVED deadline 1011.6 ms | held 200 ms; backlog 189.8 ms; 925 ms; deadline 965.5 ms |
| 100 ms delay-target step down | rate 1.0000 to 1.2000; largest frame-to-frame rate change 0.02126; DERIVED limit 0.06411 | rate 1.0000 to 1.2000; largest change 0.19999 |

## Tests

In `src/net/view_clock/pacing.rs`, each mounting `view_clock::install`. Expected values are
derived in the test (`expected_bandwidth` is `e v / T` from the noise-bound argument, never the
implementation's `bandwidth`). Observation lengths come from W, v and the derived recovery bounds,
and every test asserts its frames reach the last deadline it checks:

- `irregular_production_plays_at_a_regular_rate`: the band above, no holds or cuts.
- `a_regular_server_at_any_followable_speed_plays_without_holds_or_lead`: speeds 0.5 to 1.1; no
  hold or cut; lead within the target's phase range plus quantization; rate within `v / T` times
  that range of s.
- `a_server_speed_change_settles_once_the_window_turns_over_and_every_stop_is_a_hold`: steps 1.0 to
  0.5, 1.0 to 0.9 and 0.5 to 1.0. Every frame that reaches the newest completed tick more than a
  hundredth of a tick ahead of its target is held (25 such frames). No hold or cut after the
  DERIVED turnover.
- `a_released_server_stall_holds_once_then_catches_up_at_the_slew_limit`: one hold episode; no
  cut; catch-up never exceeds `1+v`; within `v / omega` of the target by `B / v` plus two frames.
- `a_delay_target_step_is_absorbed_smoothly_within_the_slew_limit`: no hold or cut; rate within
  `[1, 1+v]`; frame-to-frame change at most `3 v^2 F / T`; absorbed within the slew time plus
  `2 / omega` and two frames.
- `irregular_production_coverage_observations`: prints the coverage tables; asserts only the class
  invariants.

Mutants, MEASURED:

- Main's per-segment rule fails the regular-rate band (rate 0.849 against `1 +/- 0.0540`) and the
  delay step (a rate jump). It also fails the speed-change test, whose fixture reaches the newest
  tick ahead of its target only under a lagging loop.
- The nominal feedforward (the committed loop) fails the speed sweep (2.915 ms lead at 0.5) and the
  speed-change test (holds at 0.5 after the window turned over).
- The time-domain hold flag alone (the committed `held`) fails the speed-change witness (a stop at
  26.783 s ahead of the target, not reported).

The stall test passes under all three, as it should: every rule catches up at v. Every
pre-existing `net::view_clock` test passed under each mutant; they pin monotonicity, endpoints and
the `(1+v)` cap. The outage test caught a hold the direct flag alone missed: one long frame
converges exactly onto a clamped target. That is why `held` is the union of both stops.

## Validation

All compiling commands ran through `scripts/serial-cargo` with `--features dev_ui,bitprobe` and
`OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`; no window or capture was opened. The candidate
comparisons (feeds, releases, the per-segment and nominal references) ran once under a temporary
switch, deleted before the commit; the mutants ran under a temporary environment hook, also
deleted. After the merge of `main` (`PROTOCOL_REV` 106), `net::protocol` passed (19 tests).
The wire is untouched: `WIRE_TYPE_DEFS` and `PROTOCOL_REV` are unchanged by this lane.
