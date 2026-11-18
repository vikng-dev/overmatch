# View clock pacing: issue 396

Subject: Overmatch's multiplayer tank game, netcode presentation timing; game state only. No damage
model is touched. Lane `lane/view-rate`, merged with `main` at `5e82f064` (`PROTOCOL_REV` 106).
The law is [the 2026-11-18 design](view-pacing-law-2026-11-18.md), built exactly; this report
measures its SIMULATED rows. The contract is
[net-motion-contract.md](../docs/design/net-motion-contract.md).

## The law

`ViewClock::advance`, `ViewClock::learn` and `ViewClock::pace` in `src/net/view_clock.rs`:

- The target point is `S^-1(clamp(now - Delta, first stamp, newest stamp))`; `Delta` slews toward
  `median interval + max_W(x)` at v per Real second, unchanged.
- The stream rate q is filtered from the map's mean slope over each advance of the target point,
  gain `1 - e^(-omega dtau)`, clamped to `[0, 1+v]`; a hold does not advance the point and freezes
  q. The first cursor frame reads the slope across the retained map. Neither the cursor nor the
  statistics window enters q; `window_rate` is deleted.
- The cursor's rate u is stored absolute. Each frame applies the exact critically damped step
  about q at `omega = v / (kappa T)`, then clamps u to `[0, 1+v]` and the advance to
  `[0, (1+v) h]`.
- `held` is one condition: the rate-limited request passed the newest completed tick by more than
  one U0F16 cursor quantum. The frame after a hold or a cut, and the first frame, start from u = q.
- The cursor and the inverse are exact f64 inside the clock; only `ViewClock::now` rounds to U0F16.
  A backlog above `max(view_cut_s, admission_s)` still cuts.

`kappa = (sqrt 13 - 2) e^(-(5 - sqrt 13)/2) + (sqrt 13 + 2) e^(-(5 + sqrt 13)/2) = 0.87536`
(DERIVED), so `omega = 14.623 /s`, `1/omega = 68.39 ms` for v = 0.2, T = 15.625 ms. It is the
`Resolution` in `bandwidth`, with the L1 derivation beside it: with the filter and the loop both
at omega the rate's response to target phase is `omega^2 (3 - 5x + x^2) e^-x`, of zero area and
L1 norm `2 kappa omega`, so a tick of production phase moves the rate by at most v.

## Design rows, SIMULATED to MEASURED

Headless installer fixtures in `src/net/view_clock/pacing.rs`: 120 Hz frames, 40 ms constant
link, v = 0.2, T = 15.625 ms, W = 10 s; 60 s measured after two windows of settling. "Main" is the
segment-by-segment rule with its time-domain hold flag and release from rest.

| Row | DERIVED bound | SIMULATED | MEASURED |
|---|---|---|---|
| R1 jittered, lateness slack | `1 +/- 0.0525` (v·(spread + applied range)/T + quantization) | 0.9716–1.0265, 0 holds | 0.9747–1.0293, 0 holds (main 0.8031–1.2000) |
| R1 bang-bang phase, 60–1000 Hz frames | 0.05 at p = T/4 | 0.0480–0.0499 | not re-measured |
| R2 regular 0.5 to 1 + v/2 | rate = s, lead within the delay's phase + 1 quantum | 0.5–1.19: rate = s, 0 holds, lead 0.000 ms | 0.5–1.1 by 0.1: rate = s to 4 decimals, 0 holds, lead within ±0.0001 ms |
| R3 speed step 0.5→1, lag peak | `0.840 |d| / omega` = 28.72 ms at 111 ms | 27.2 ms at 108 ms | 27.106 ms, 0.183 s after the switch's stamp, which the target meets one applied delay later |
| R3 speed step, rate change per frame | 0.04883 | 0.048 | 0.04749 |
| R3 delay slew (100 ms target step), rate change per frame | 0.00908 | 0.009 | 0.00896 |
| R3 delay slew, lag | `2 v / omega` = 27.35 ms | 26.4 ms | 26.441 ms |
| R3 1 s capped stall, settled rate | `v · applied range / T` = 0.01073 (applied delay moved 0.833 ms) | rate 1.00000 | deviation 0.004477, 0 cuts |
| 200 ms stall: overshoot after release | one quantum (0.00024 ms) | 0.0000 ms | 0.0001 ms |
| 200 ms stall: within 1 ms of target after the peak | DERIVED deadline 1213.8 ms | 1.09 s (main 0.82 s) | 1075 ms |
| Release first-frame rate | `[q, q + omega^2 |z0| F / 2]` = [1, 1.02326] | — | 1.02141 (main 1.2) |
| Frame lengths 0, 2–60, 250 ms | monotone, never past newest, ≤ (1+v)h + 2 quanta | holds | holds; 73 cuts, at 250 ms frames (main 74) |
| After a cut, lead | `0.54 v / omega` = 7.4 ms | 4.3 ms | not measured |

R2's held frames were the one counterexample on the way. With the cursor stored as a rounded
`TickInstant`, a regular server at 0.7 held 96 frames at the zero-slack link, its lead 1 to 5
quanta: within the derived lead bound, but at zero slack one quantum of lead at a contact is a
hold, and the rounding fed back into the loop as phase. The design's `c_prev` is exact; keeping
the cursor and the inverse exact inside the clock removed every such hold (0 at every speed, lead
±0.1 µs). The derivation was right; the implementation had not been.

## Speed changes

Last hold after the step; held frames; constant link (zero slack). Main MEASURED on the same
fixture.

| Step | Main | Law, SIMULATED | Law, MEASURED | DERIVED no hold after |
|---|---|---|---|---|
| 1.0→0.5 | 6.758 s; 430 | 6.883 s; 116 | 6.883 s; 116 | 7.815 s |
| 1.0→0.75 | 5.767 s; 138 | 5.808 s; 140 | 5.767 s; 139 | 6.835 s |
| 1.0→0.9 | 5.300 s; 76 | 5.508 s; 29 | 5.508 s; 29 | 6.378 s |
| 0.5→1.0 | none | none | none | none |

R3 yields as the design states: a slowdown's last hold may trail main's by up to `14.4 / omega`
(DERIVED 0.98 s), MEASURED +0.125 s at 0.5, 0 at 0.75 and +0.208 s at 0.9. While the target
decelerates, a law within the phase bound leads it, and at zero slack any lead is a contact.

## Estimator coverage

Held frames over 60 s. The delay target covers lateness, not production phase, so holds at a
zero-slack link remain until the estimator changes (owner's decision; out of scope).

| Production | Main | Law, SIMULATED | Law, MEASURED |
|---|---|---|---|
| alternating, constant link | 1440 | 480 | 480, rate 0.7938–1.0260 |
| jittered, constant link | 294 | 187 | 177 |
| alternating, lateness slack | 16 | — | 15 |
| jittered, lateness slack at any phase | 4 | — | 5 |

## Release, and the rejected states

Release is a rate step from 0 to q by decision: a hold is a gap in facts, not a change in the
stream's speed, and from zero rate deviation the lead `z0 (1 + omega t) e^(-omega t)` never turns
positive. Both rejected states ran once under a temporary hook, deleted before the commit:

- **Rest (u = 0):** loses up to `q / (e omega)` = 25 ms after every hold and dips to 0.116; it
  holds less (160 against 480 alternating, 30 against 116 at 1.0→0.5) only by adding delay
  (lag 18.2 ms against 7.8 ms at 1.0→0.5).
- **u = q - omega z:** the fastest admissible approach; it holds more, 1920 against 480
  alternating and 298 against 116 at 1.0→0.5, and its first released frame runs at 1.198
  against the derived ceiling 1.023.

## Cut margin: the capped stall cuts

The design expected 0 cuts on the real-UDP 1 s capped stall. MEASURED, it cuts once, and the
owner accepted that cut; the expectation and its cause are stated in the motion contract.

| 1 s capped stall | Law | Main |
|---|---|---|
| Real UDP: peak backlog at the cut gate | 263.73 ms | 239.06 ms |
| Real UDP: cuts | 1, at 1.078 s, 16.88 ticks | 0 |
| Headless: peak backlog at the cut gate | 248.13 ms | 218.83 ms |
| Headless: cuts | 0 | 0 |

The ramp costs +24.7 ms over UDP and +29.3 ms headless (MEASURED), against the design's predicted
20–25 ms. The law, `view_cut_s` and the release state are unchanged.

## Stall and spike re-measurements

`src/net/view_clock/udp.rs`, real loopback UDP, production protocol and runner schedule, 80 ms RTT
with integer 10 ms downlink jitter, Fixture 12 ms ordinary work, 128 Hz reads. MEASURED. Slow and
fast count frames with a nonzero rate below 0.99 or above 1.01.

| Scenario | Held | Cuts | Peak gate backlog | Delay, baseline to peak | Slow s (main) | Fast s (main) |
|---|---|---|---|---|---|---|
| 200 ms work stall | 179.6875 ms | 0 | 197.16 ms (main 176.56) | 70.3125 to 250 ms | 0.6953 (0.4453) | 1.7109 (1.3438) |
| 1 s capped work stall | 984.375 ms | 1, at 1.078 s, 16.88 ticks | 263.73 ms (main 239.06) | 70.3125 to 1046.875 ms | 5.1094 (4.2734) | 5.2578 (5.4844) |
| 500 ms downlink blackout | 484.375 ms | 1, at 0.5 s, 31.4 ticks (main 1) | 490.63 ms | 70.3125 to 554.6875 ms | 2.7109 (2.4141) | 2.7188 (2.4219) |

The settled 80/10 link, the 60 ms burst lever and the loaded-server distribution show zero holds
and cuts; the applied delay on 80/10 is 70.3125 ms, inside its DERIVED band.

## Tests and mutants

`src/net/view_clock/pacing.rs`, each mounting `view_clock::install`; every bound is computed in the
test from v, T, W and kappa's closed form (`expected_bandwidth`, never the implementation's
`bandwidth`). Mutants ran once each under a temporary environment hook, deleted before the commit;
every listed test failed at the named assertion, and every other `net::view_clock` test passed.

| Mutant | Killed by |
|---|---|
| Main's rule (per segment, time-domain flag, release from rest) | `irregular_production_plays_at_a_regular_rate` (0.8031), `a_server_stall_leaves_the_rate_unchanged_across_the_statistics_window` (0.1000), `playback_acceleration_stays_within_the_derived_law` (0.375 per frame), `hold_flags_exactly_requests_past_the_newest_tick`, `a_released_hold_resumes_at_the_stream_rate_and_closes_its_backlog_at_the_slew_limit` (1.2) |
| Release from rest | the release test (first rate 0.136) |
| Release at q - omega z | the regular-speed test (holds at 0.5), the speed-change test (hold at 7.850 s), the acceleration test (delay step), the release test (1.198) |
| q from the window's rate | the speed-change test (hold at 7.817 s), the stall test (0.147), the release test (1.24 ms overshoot) |
| q fixed at 1 | the regular-speed test, the speed-change test, the acceleration test, the release test (1.53 ms overshoot) |
| u stored relative to q | the acceleration test (speed step) |
| Flag also on a clamped target | the hold-flag test (a completed request onto a clamped target) |
| Time-domain flag only | the hold-flag test (same state) |
| omega = e v / T (round 2's constant) | the band test (0.9455), the acceleration test, the release test (1.109) |

Main's arm on the exact cursor also tripped the monotonic `assert!` in several tests, because it
rounds the cursor before its `max`; those failures are the emulation's, so main's row lists the
failures measured with its own rounded cursor. No mutant here breaks
`any_frame_length_keeps_the_cursor_forward_behind_facts_and_within_the_slew_limit`; it pins the
clamps, which no candidate changes. `irregular_production_coverage_observations` prints the
coverage table and asserts only the class invariants.

Fixtures follow the timing data they depend on. The released-stall fixture is 0.8 of the
continuity floor (200 ms as shipped), and the stall fixture resumes production at the end of
the silence when the cap admits no whole tick. The hold-flag proof places its newest tick from
the floor, so its forced cut stays a cut under any floor, and sizes its rising-delay frame as
`T / (2 - v)`, which keeps the target short of the newest tick and the request past it at any
`v` in `(0, 1)`. `assets/net_timing.ron` refuses `view_rate` outside `(0, 1)` at load, naming the
file, the field and the fix; `view_rate_outside_zero_to_one_is_refused_at_load` covers 1, 1.5,
0, -0.2, NaN and infinity and loads `1 - EPSILON`.

Stamps are outside input. A positive frame-start interval too short for u32::MAX ticks times T
over it to stay finite is refused at admission as `StreamRefusal::Interval`, through the same
visible match close as an out-of-order stamp, so the stream rate is finite by construction.
`a_stamp_interval_too_short_to_time_is_refused_before_pacing` uses the review's counterexample:
stamps 0, 0 and the smallest positive f64. MEASURED once with the check disabled under a
temporary hook, since deleted: the third stamp is admitted, q is 1.2 on the first frame and NaN
on the second, and `requested.min(newest)` moves the cursor from tick 2 to the newest tick 3 in
one frame, unpaced and not held; the test fails at its refusal assertion. The hold-flag proof
also ran once at v of 0.01, 0.1, 0.9 and 0.999 and at floors of T, 0.6 s and 1 s, and passed.

## Validation

All compiling commands ran through `scripts/serial-cargo` with `--features dev_ui,bitprobe` and
`OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`; no window or capture was opened. After the merge of
`main` and again after the review fixes: `cargo fmt --all --check`, CI clippy, the server-only
build, the targeted `net::` and `tests::doc_citations` tests (including `net::protocol`) and the
full library suite passed.
