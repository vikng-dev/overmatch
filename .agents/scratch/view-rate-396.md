# View clock pacing: issue 396

Subject: Overmatch's multiplayer tank game, netcode presentation timing; game state only. No damage
model is touched. Lane `lane/view-rate`, on `main` at `49004357`.

## What changed

The cursor no longer follows the inverse of the stamp map segment by segment. That rule turned
each server tick's production interval into playback speed: a short interval asked for fast
playback and rode the `(1+v)` clamp, a long one slowed at once. It is deleted, together with the
`.max(previous)` that silently enforced monotonic time; monotonicity now follows from the law
and is asserted.

The cursor advances at the nominal one simulation second per Real second, steered by a
critically damped phase loop toward the same inverse-map target `S^-1(now - Delta)`
(`ViewClock::pace`, `src/net/view_clock.rs`):

- `dc/dt = 1 + r`, `dr/dt = -omega^2 z - 2 omega r`, where `z` is cursor minus target in
  simulation seconds and `r` the rate deviation (new `ViewClock::rate` state).
- Exact over any frame length: each frame takes the target as moving at the nominal rate and
  applies the closed-form critically damped step, so a 1 s client stall is as stable as a
  7.8 ms frame.
- Limits kept: the frame's advance stays in `[0, (1+v) dt]` and `r` in `[-1, v]` (forward-only,
  catch-up at most `1+v`); never past the newest completed tick (a hold stops the cursor and sets
  `r = -1`, so release accelerates from rest rather than from a rate the hold never allowed); a
  backlog above `max(view_cut_s, admission_s)` still cuts to the target with `r = 0`; waiting
  before the first stamp pair and epoch resets are unchanged (`rate` resets with the clock).
- The delay estimator `Delta* = median interval + max_W(x)` and its slew of v per Real second are
  unchanged. The slew turns a step in `Delta*` into a slope change of v; the loop turns that slope
  change into a smooth rate change.

### The constant, derived

`omega = e v / T` (`bandwidth`), DERIVED from the owner's `view_rate` Taste v = 0.2 and the
authority tick T = 15.625 ms: omega = 34.79 per second, 1/omega = 28.7 ms. It is a `Resolution`
number with this stability reason:

- Critical damping (zeta = 1): the cursor never overshoots a target that sits one stream interval
  behind the newest completed fact, so the loop never spends buffer the estimator sized.
- The rate response to target phase has impulse response `omega^2 (1 - omega t) e^(-omega t)`,
  L1 norm `2 omega / e`, integral zero. Any target phase of peak-to-peak spread `p` moves the rate
  by at most `(omega / e) p = v p / T`. `omega = e v / T` is the widest band for which production
  phase noise within one authority tick never reaches the slew limit; larger phase steps (released
  backlogs) saturate at v, as recoveries should. `e` is the law's form constant.

Consequences, DERIVED from the law:

- A slewing delay changes the rate by at most `v omega / e = v^2 / T` = 2.56 per second; the rate
  reaches the full v over about 115 ms instead of in one frame.
- A released backlog B closes to within `v / omega = T / e` = 5.75 ms of the target in `B / v`:
  the rate saturates at v until `2 v / omega` remains, and the critically damped exit leaves
  `4 e^-2 v / omega` at that time.
- A sustained server slowdown `delta` (server rate `1 + delta`) leaves the cursor `2 |delta| /
  omega` ahead of its target: 5.7 ms at a 10 % slowdown. The per-segment rule had no such lead.
  When the lead exceeds about one stream interval, holds return; that is a server below about
  `1 - e v / 2` = 73 % of real time, an over-budget regime.

No new authored number, switch or data field exists. The wire is untouched: `WIRE_TYPE_DEFS` and
`PROTOCOL_REV` are unchanged.

## Reproduction and rate distributions

Fixture (`src/net/view_clock/pacing.rs`): the shipped installer, fed completed stamps through the
replication completion path; 120 Hz client frames; 40 ms constant one-way completion delay;
production phase spread a quarter tick (3.906 ms) peak to peak, either alternating on schedule and
late (intervals 0.75 T and 1.25 T) or a per-tick uniform draw. 60 s measured after two windows of
settling. Per-frame rate is cursor advance in simulation seconds over the frame's Real duration.
All values MEASURED. "Before" is `main`'s per-segment rule; "after" is this lane.

Constant link (the reproduction the brief asked for; holds included in the distribution):

| Production | Code | min | p1 | p5 | p50 | p95 | p99 | max | at 1+v | outside 1 % | held frames |
|---|---|---|---|---|---|---|---|---|---|---|---|
| regular | both | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 0 | 0 | 0 |
| alternating | before | 0.3417 | 0.3417 | 0.3417 | 1.0750 | 1.2000 | 1.2000 | 1.2000 | 46.7 % | 93.3 % | 1440 / 7200 |
| alternating | after | 0.2612 | 0.2612 | 0.2612 | 1.0885 | 1.1672 | 1.1672 | 1.1672 | 0 | 100 % | 480 / 7200 |
| jittered | before | 0.6114 | 0.8206 | 0.8658 | 0.9963 | 1.1555 | 1.2000 | 1.2000 | 2.2 % | 88.4 % | 294 / 7200 |
| jittered | after | 0.2429 | 0.2516 | 0.8575 | 1.0083 | 1.1314 | 1.1497 | 1.1746 | 0 | 62.4 % | 102 / 7200 |

At a constant link both rules hold: the estimator does not cover production phase (next section).
The distributions there are dominated by holds and their recoveries, so they do not isolate pacing.

Lateness slack (every 61st tick completes a tick late, so the window's lateness maximum leaves
every other tick a tick of slack); the asserted test uses jittered production with the late
ticks on schedule:

| Production | Code | min | p1 | p5 | p50 | p95 | p99 | max | at 1+v | outside 1 % | held frames |
|---|---|---|---|---|---|---|---|---|---|---|---|
| jittered, late ticks on schedule (asserted) | before | 0.8031 | 0.8264 | 0.8636 | 0.9943 | 1.1640 | 1.2000 | 1.2000 | 2.6 % | 88.9 % | 0 |
| jittered, late ticks on schedule (asserted) | after | 0.9652 | 0.9771 | 0.9819 | 0.9997 | 1.0184 | 1.0246 | 1.0405 | 0 | 40.0 % | 0 |
| jittered, late ticks at any phase | before | 0.8058 | 0.8277 | 0.8649 | 0.9947 | 1.1605 | 1.2000 | 1.2000 | 2.4 % | 88.8 % | 4 |
| jittered, late ticks at any phase | after | 0.2483 | 0.9768 | 0.9824 | 0.9999 | 1.0193 | 1.0311 | 1.1561 | 0 | 40.4 % | 4 |
| alternating | before | 0.6400 | 0.8000 | 0.8000 | 0.9833 | 1.2000 | 1.2000 | 1.2000 | 20.1 % | 99.6 % | 16 |
| alternating | after | 0.2560 | 0.9774 | 0.9924 | 0.9989 | 1.0118 | 1.1173 | 1.1575 | 0 | 12.8 % | 8 |

The asserted band for the first row is DERIVED as `1 +/- (omega / e)(spread + applied-delay
range) + quantization` = `1 +/- 0.0525` (applied delay range 0.190 ms). The extremes of the other
"after" rows come from holds and the release from rest that follows them.

## Not anticipated by the design

**The delay target covers lateness, not production phase.** A smooth cursor presents the mean
production line, so it needs `Delta >= T + max(d + x) - mean(d)` for production phase `d` and
lateness `x`; the per-segment rule needed the longest interval instead of the median. The target
`median + max_W(x)` leaves the tick that sets the lateness maximum short by its production phase
above the mean, and at a constant link every tick is such a tick. Under alternating production the
nearest-rank median is the short interval about half the time (the delay target swung
51.299–59.114 ms, the full `2 x 0.25 T`), so holds remain even with lateness slack. The pacing loop
needs less margin than the old rule (`mean` instead of `longest`), which is why holds fell from
1440 to 480 and from 294 to 102 frames, but it cannot remove them. The brief keeps the estimator;
changing it is an owner decision. Candidates: a margin of `T` plus the production phase spread over
W, or lateness measured from the production line rather than each tick's own stamp. Either needs a
treatment for stalls, which move the phase; the median exists to reject them.

Real links have jitter, which leaves typical ticks slack below the lateness maximum, so holds there
need a late tick and a late production phase together; the issue does not report holds. The
hosted capture in the issue's done-when will show whether they matter.

**Slow servers lead.** See the derived consequence above: the nominal-rate loop runs
`2 |delta| / omega` ahead under a sustained slowdown and holds below about 73 % server speed.

## Stall and spike re-measurements

`src/net/view_clock/udp.rs`, real loopback UDP, production protocol and runner schedule, 80 ms RTT
with integer 10 ms downlink jitter, Fixture 12 ms ordinary work, 128 Hz reads. MEASURED; the
"before" column is the per-segment rule on the same fixture, matching
[the slice 2 report](view-clock-slice-2.md).

| Scenario | Held | Cuts | Delay (baseline to peak) | Above baseline | Slow s (before) | Fast s (before) |
|---|---|---|---|---|---|---|
| 200 ms work stall | 179.6875 ms | 0 | 70.3125 to 250 ms | 10.8984375 s, until 11.1484375 s | 0.5625 (0.4453125) | 1.4921875 (1.34375) |
| 1 s capped work stall | 976.5625 ms | 0 | 70.3125 to 1046.875 ms | 14.8828125 s, until 15.9296875 s | 4.390625 (4.2734375) | 5.6328125 (5.484375) |
| 500 ms downlink blackout | 484.375 ms | 1, at 0.5 s, 31.4 ticks | 70.3125 to 554.6875 ms | 12.421875 s, until 12.9140625 s | 2.546875 (2.4140625) | 2.546875 (2.421875) |

Holds, cuts and delay are unchanged. Slow and fast mean a nonzero frame rate below 0.99 or above
1.01; the loop's smooth onset and landing put about 0.1 to 0.15 s more frames in those classes
per scenario. The settled 80/10 link, the 60 ms burst lever and the loaded-server distribution
still show zero holds and cuts; the applied delay on 80/10 is 70.3125 ms, inside its DERIVED band.

Headless installer fixtures, MEASURED:

| Scenario | After | Before |
|---|---|---|
| 200 ms server silence, then admitted debt | held 200 ms; peak backlog 198.8 ms; within 5.75 ms of the target 975 ms after the peak; DERIVED deadline 1010.7 ms | held 200 ms; backlog 189.8 ms; 925 ms; deadline 965.5 ms |
| 100 ms delay-target step down | rate 1.0000 to 1.2000; largest frame-to-frame rate change 0.02126; DERIVED limit 0.06411 | rate 1.0000 to 1.2000; largest change 0.19999 |

## Tests

New, in `src/net/view_clock/pacing.rs`, each mounting `view_clock::install`:

- `irregular_production_plays_at_a_regular_rate`: the derived rate band above, no holds or cuts.
- `a_released_server_stall_holds_then_catches_up_at_the_slew_limit`: the silence holds; no cut;
  catch-up never exceeds `1+v`; within `v / omega` of the target by `B / v` plus two frames.
- `a_delay_target_step_is_absorbed_smoothly_within_the_slew_limit`: no hold or cut; rate within
  `[1, 1+v]`; frame-to-frame change at most `3 v^2 F / T`; absorbed within the slew time plus
  `2 / omega` and two frames.
- `irregular_production_coverage_observations`: prints the tables above; asserts only the class
  invariants.

Mutant, MEASURED: restoring the per-segment rule fails the first (rate 1.2 against a 0.0525 band)
and the third (frame-to-frame change 0.19999 against 0.06411). The stall test passes under both
rules, as it should: both catch up at v. Every pre-existing `net::view_clock` test passed under
both rules; they pin monotonicity, endpoints and the `(1+v)` cap, which the old rule also met.

## Validation

All compiling commands ran through `scripts/serial-cargo` with `--features dev_ui,bitprobe` and
`OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`; no window or capture was opened. After the last source
edit, MEASURED:

| Check | Result |
|---|---|
| `cargo fmt --all --check` | passed |
| CI clippy, locked workspace, all targets, warnings denied | passed |
| `check --no-default-features --bin overmatch-server` | passed; the two existing audio-only dead-code warnings remain |
| `net::` and `tests::doc_citations` | 430 passed, 7 ignored, 0 failed |

The full library suite was not run; the wire is unchanged, so CI's run after the push covers it.
API facts checked in the pinned sources: lightyear_core 0.31.0 `TickInstant` (U32F32) and
`Overstep::from_f32` (saturating U0F16, monotone), bevy_time 0.20.1 `ManualDuration` and the
zero-delta first `Time<Real>` sample, lightyear_replication 0.31.0 `ReplicationCheckpointMap`.
