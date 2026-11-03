# Fixed client input delay

Subject: the game's client input, fire presentation and netcode; game state only.

Decision: retain the encoder-range assertion and make `shipping_input_delay` independent of
latency. No buffer clearing, panic recovery, runtime lever, wire change or protocol bump.

## Source law and regression

VERIFIED against the installed Lightyear 0.30.1 source, `lightyear_sync::timeline::input`:
`fixed_input_delay(d)` leaves a finite prediction allowance. `input_delay_ticks` adds the sync
jitter margin to RTT, divides by the tick duration, rounds up, and casts the float to `u16`.
Rust saturates that cast; it does not wrap. Equal minimum/maximum delay alone therefore does
not make the helper fixed above its allowance.

The shipped config sets both delay bounds to `d` and the allowance to `u16::MAX - d`.
DERIVED: the allowance sum is exactly `u16::MAX`, without integer overflow; every representable
RTT tick count either returns the minimum or the maximum, both `d`. The allowance is an adapter
for this upstream calculation, not gameplay prediction; Lightyear's prediction feature is disabled.
Using `u16::MAX` alone would overflow the upstream sum. Setting the allowance to zero selects
lockstep, adds extra RTT ticks, and makes delay track latency.

The pinned `InputTimelineConfig` has no shift-recompute flag. Its installed observer remains:
recomputing the shipped config is idempotent. `sync_margin::InputSyncBase` retains the same
config during margin rewrites. `assert_input_buffer_ranges` is unchanged.

The closest public path to private `input_delay_ticks` is the actual config-insert observer,
with a client topology and `Link.stats`, followed by the installed shift observer. Tests mount
`ClientPlugins` and the shipped native-input protocol, rather than copying the delay law.

MEASURED before the fix, using
`scripts/serial-cargo test --lib --features dev_ui,bitprobe -- input_delay_is_constant native_timeline_shift_preserves_the_input_buffer`:

- The bad-link fixture returned 925 ticks instead of the shipped three ticks.
- Link recovery plus a backward shift panicked with buffer start `Tick(945)` beyond encoder
  end `Tick(23)`, the reported crash's invariant.

Both tests pass with the fix. Restoring `fixed_input_delay` is the mutant that turns both red.
The config test also crosses RTT-tick saturation and reaches the largest effective RTT that
`Duration` can represent, plus extreme jitter. The shift test preserves the buffer, remote mark
and original snapshot provenance, then writes the next tick; clearing the buffer cannot pass it.

Limit: upstream multiplies/adds `Duration` values before the saturating cast. A raw RTT already
at `Duration::MAX` plus a positive margin, or an overflowing jitter product, still panics there.
No `InputDelayConfig` can change that earlier arithmetic. This fix covers every representable
*effective* RTT, including saturation; it does not claim to repair those upstream duration overflows.

## Read-only: the much larger sync margin

MEASURED on 2026-11-03 from the existing [Battle verification](../battle-verify/report.md)
client log retained in bay-3's `target/battle-verify/`:
at 11:41:05.845882 UTC the floor-plus-coverage was 11169.87 ticks, deadband zero, arrival
spread 435.6 ms, and sample count 19858. The adjacent interpolation log reported RTT 35.2 ms.
The panic frame then logged 11140.04 ticks. This is estimator coverage, not an RTT measurement.

`ArrivalDelay::record` samples only advancing remote ticks. Its eviction ring holds samples,
not elapsed time. MEASURED sample 16018 was logged at 11:26:32.644106 UTC, sample 16020 at
11:26:33.131694, and sample 17938 at 11:33:46.787517. DERIVED from the ring's 3840-sample
capacity: the oldest retained sample at count 19858 is bracketed between those first two times,
so retained history spans 872.714–873.202 seconds, rather than its nominal 60 seconds.

`coverage = max(Qp - Q50, 0)`, while the recent minimum uses a two-second window.
DERIVED at the first timestamp: coverage is `(11169.87 - 0.5) / 64 = 174.5214` seconds;
combining it with `Qp - recent_anchor = 0.4356` seconds puts the median about 174.0858 seconds
below that recent anchor. `bulk` clamps its negative difference to zero, explaining the zero
deadband display. The sparse sample ring compares minutes-old accumulated clock drift with
the current stream. `record` explicitly includes sender pacing in its wall-time-minus-tick
sample; the Battle report independently records the server clock falling behind wall time.
The source and logs explain the large coverage; they do not identify the cause of the slowdown.

Priority follow-up: the estimator's retention and treatment of clock-rate drift. Fixing input
delay does not remove this margin from the sync objective. No estimator or server behavior
changed here, and the full Battle was not rerun.

## Read-only: fire, aim, and other constant-delay assumptions

Lightyear's input objective subtracts input delay; encoding adds it back. DERIVED: changing the
delay does not cancel the oversized coverage in the final authored input tick. Its server
`receive_input_message` drops messages outside the fixed ±64-tick acceptance window before
writing the buffer. At a sustained margin of this size, new fire and aim commands are therefore
out of range, not queued for eventual execution. This is source-derived behavior, not a replay
of the Battle's individual inputs; that run did not trace these messages or timeline deltas.

On missing input the native backend holds the previous snapshot. `protocol::attest` suppresses
both fire inputs when provenance names another tick, so stale input cannot keep firing; aim and
other level values can retain their last accepted state. Local device gathering and aim memory
continue separately, so the view can disagree with what the authority accepts.

The old adaptive delay can also outgrow `fire_presentation::AuthoredIntent` retention, evicting
pending local fire intent before its presentation tick. Fixed delay removes that growth.
A separate source-derived shift hazard remains in that ledger: its tick
keys and `presented_through` do not observe timeline shifts. A backward shift can suppress fresh
records until their stamps exceed the old tail/frontier. A forward shift can latch skipped
primary edges, while secondary fire requires an entry for the exact current tick. Native buffer
indices do shift, but their snapshot provenance intentionally does not; attestation rejects
consumables whose original stamp no longer names their filed tick. These presentation/clock
questions were inspected only, not changed.

The net-tree audit found no second production delay config owner: startup and the hosted
harness use `shipping_input_delay`; sync-margin rewrites retain it by value. Stamping and the
encoder guard both read `LocalTimelineSync::input_delay`, matching Lightyear's buffer/encoder
path. Tests in input cadence and respawn flow read the declared shipped delay. The remaining
calls to upstream `fixed_input_delay` in net tests deliberately exercise changing or zero delay.

## Ratchet and gates

Closed net-19: the accepted duplicate horizon now derives from the catch-up window owner and
the harness timestep under labelled fixture traffic assumptions. DERIVED current horizon is
unchanged at 1200 ids.
Closed net-20: reconnect policy is required provenance-bearing `assets/net/client.ron`, parsed
and checked before client startup. MEASURED by comparing the previous constants with the data:
give-up, base, step, cap and hint threshold remain 30 s, 1 s, 0.5 s, 5 s and three attempts.
There is no new environment variable or runtime configuration path.

PASS: per-file rustfmt, `cargo fmt --all --check`, and
`scripts/serial-cargo test --lib --features dev_ui,bitprobe -- net:: doc_citations ui_ascii net_boundary bake::`:
MEASURED 382 passed, none failed, one live hosted test ignored. PASS: CI clippy with `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
All builds/tests used the machine queue, unattended/headless environment, and this lane's bay.
No standalone game binary, capture, pricing run, issue comment or push was performed.
