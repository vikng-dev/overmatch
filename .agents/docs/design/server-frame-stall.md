# Server frame starvation under overload

Subject: the video game's server scheduler during environment destruction; game state only.

Status: decided by the owner on 2026-11-05, until a revisit before the Steam release (#279).
- **Overload policy:** catch up, the library default (option 1 in the table below), with every
  over-budget tick visible.
- **Ticks over budget are bugs:** a tick over its 15.625 ms budget is a cost bug to fix at its
  source, never a load the game tolerates as part of play. That includes a building collapse.
- **One clock:** the tick is the dedicated server's only pacing clock (#278,
  `src/net/server_runner.rs`). The runner admits a batch of missed ticks under the library's
  250 ms cap and spends it one tick per frame ("One tick per frame during catch-up" below).

## Finding

MEASURED: individual fixed ticks hold the game server for seconds in construction
weight assessment. DERIVED: Bevy's finite catch-up batch multiplies the interval
before frame work resumes. The hypothesized unbounded per-frame catch-up loop is
not present. Lowering the batch limit alone cannot solve the observed single-tick
stall.

The sampled path is `construction::breakage::apply_deposits` -> `evaluate_owner` ->
`run_laws` -> `construction::fracture::weigh`, including `held` and `force_wrench`.
The relevant source entries are `src/construction/breakage.rs:1645`,
`src/construction/breakage.rs:1509`, `src/construction/breakage.rs:2141`,
`src/construction/fracture.rs:2259` and `src/construction/fracture.rs:2970`.
These are hot-stack observations, not a diagnosis of which law evaluation can be
removed or cached without changing game state.

## What the clock actually bounds

DERIVED from the locked sources (`Cargo.lock`: Bevy 0.19.1, Lightyear 0.30.1):

- `src/net/server.rs`, `run`, mounts `server_runner::plugin`: Lightyear's
  `ServerPlugins { tick_duration: TICK }` and a custom Bevy runner. The fixed step is
  15.625 ms (Resolution: the existing authority integration cadence).
- Bevy `bevy_app/src/schedule_runner.rs`, `ScheduleRunnerPlugin::build`, supplies
  the custom runner's plugin-finish/cleanup and exit-handling pattern. `App::update`
  remains synchronous; a deadline cannot interrupt work already running.
- Bevy `bevy_time/src/virt.rs`, `Time<Virtual>::default`, caps the wall delta admitted
  to virtual time at 250 ms. `advance_with_raw_delta` discards excess elapsed wall
  time; this is already slow simulation under sufficiently severe load.
- Bevy `bevy_time/src/fixed.rs`, `run_fixed_main_schedule`, adds that delta once and
  spends the accumulator until less than a timestep remains. It never adds the time
  spent running those ticks back into the same frame's accumulator.
- DERIVED: with unchanged timestep and unit virtual speed, 250 ms is 16 ticks. That is the
  most this server admits in one batch. `net::server_runner` replaces `run_fixed_main_schedule`
  with a loop that spends a batch one tick per frame.
- Lightyear `lightyear_core/src/plugin.rs`, `CorePlugins::build`, uses Bevy's
  `TimePlugin`. `lightyear_core/src/timeline.rs`, `TimelinePlugin`, sets the fixed
  timestep and increments its local timeline in `FixedFirst`; it does not replace
  the catch-up loop.
- Bevy `bevy_app/src/main_schedule.rs`, `MainScheduleOrder::default`, orders
  `PreUpdate`, `RunFixedMainLoop`, `Update`, `PostUpdate`, then `Last`.
  Lightyear's receive/send bridge lives in `PreUpdate`/`PostUpdate`
  (`lightyear_replication/src/server.rs`, `LightyearRepliconServerBackend::build`).
  `src/net/server_stats.rs`, `install`, registers the stats emitter in `Last`.

There is a simulation-time bound, not an absolute wall-time bound. DERIVED: frame
work resumes after one complete tick plus the frame's other work. It cannot service
packets during that tick.

Every frame runs exactly one tick. Missed ticks run in the following frames, back to
back. Wall time that passes while they do waits for the next batch.

## Tick-paced authority (#278)

The runner reads the next deadline from `Time<Real>::last_update()` plus the time still
missing from a tick: `Time<Fixed>::timestep()` less the overstep and the wall time awaiting
admission. It rechecks the clock after sleeping and supplies that wake sample through
`TimeUpdateStrategy::ManualInstant`, so `Time<Real>` is wall time on every frame.
`run_one_admitted_tick` reads that frame's delta and spends one tick. There is no separately
advancing frame deadline. The dedicated server asserts unpaused, unit-speed virtual time; no
authority system changes it.

`Time<Real>::update_with_instant` primes the initial zero-delta sample without
running a frame. The first frame waits for a complete tick. After every frame the
runner recomputes from the accumulator remainder, including the time spent doing
frame work. An already-overdue deadline starts immediately. The client runner is
unchanged.

Verified against locked Bevy 0.19.1's `bevy_app/src/app.rs`, `schedule_runner.rs`,
`bevy_time/src/lib.rs`, `real.rs`, `virt.rs`, `fixed.rs`, and Lightyear 0.30.1's
`lightyear_core/src/timeline.rs`. Lightyear sets the timestep and advances its local
timeline in `FixedFirst`; it does not supply another server clock.

The controlled-clock tests mount the shipped `TickRunner` plugin and its Lightyear
plugins. Varying sub-budget frame work and scheduler oversleep produce one tick in
every frame, in receive/tick/send/last order, with no frame before its tick is due.
Separate tests inject overdue frames: each missed tick runs unslept in its own frame, a
stall past the cap admits exactly the cap, a catch-up frame's own cost is admitted after its
batch, sustained overload keeps the debt within the cap, and a silent client on real sockets
times out on wall time across a capped stall. The normal-frame test goes red with the former
4 ms runner: its first frame has zero ticks and receive/send/last without tick.
The old runner and instrumentation are absent from shipped source.

DERIVED from runtime registrations and the locked networking sources: no audited
system depends on multiple authority frames per tick:

- Netcode keep-alives use elapsed send times (default 100 ms), and timeouts advance
  from `Time<Real>::delta()`; their checks now run once per tick. Ping handling uses
  elapsed time too. Receive opportunities are tick-paced, as the owner chose.
- `server_stats` closes wall-clock periods in `Last`, with the existing possible
  one-frame overshoot; its tick and send summaries do not require tickless frames.
- `loadtest::drive` runs in `FixedUpdate`, scheduling from `Instant::elapsed()`.
- Bot commands run in `FixedUpdate`; spawn and respawn eligibility is checked in
  `Update`. Neither counts intervening frames.
- `world_reset::start_next` reconstructs synchronously at the Battle's command flush;
  it needs no intermediate frame. Hosting timers also read elapsed time.
- Two test comments cited the former server frame/tick ratio. They now state their
  actual removal-message/history invariants. The multi-frame test fixtures remain
  useful for render-driven clients and schedules that admit frames between steps.

## One tick per frame during catch-up

The owner's overload policy stands: missed ticks are caught up, and at most
`Time<Virtual>::max_delta` (250 ms, 16 ticks) of newly elapsed wall time is admitted at once.

**What is the same as Bevy's loop.** The admission law: when the last batch is spent, the wall time
elapsed since it was admitted is added to the accumulator, capped at 250 ms, and the excess is
never simulated. For the same frame costs the ticks per batch are the same.

**What is not.** A batch is spent over many frames, with receive, send and the rest of the frame
between its ticks, so that frame work lands inside the batch and lengthens it. `Time<Virtual>` no
longer feeds `Time<Fixed>`: it still advances each frame under its own per-frame cap, so the two
clocks differ by the admitted debt still unspent and by what each cap discarded. No gameplay on the
dedicated server depends on them agreeing: fixed-schedule readers of `Time` get `Time<Fixed>`, and
Lightyear's send timer has a zero interval. The dev capture's `fire_on_cue` runs in `Update` on
app seconds, as documented, so its cues can fire closer together in simulated time across a stall.
`Time<Real>` is the wall clock on every frame, so netcode's
timeouts and keep-alives count a stall in full.

MEASURED, hosted v0.7.0-alpha.11, 2 vCPU, 30 tanks, one house falling at tick 1602, two runs:
14 sends in the 199 ticks 1602 to 1800, against one per tick before. The gaps held 16 ticks each
(the cap), with a median of 387 ms and 368 ms and a maximum of 556 ms. The simulation ran at 0.60
and 0.63 of wall speed.

**Mechanism.** `lightyear_replication` `send.rs`, `update_replication_tick`, advances Replicon's
`ServerTick` once per frame after the fixed loop, and the send runs in `PostUpdate` when it changed.
`bevy_replicon` `server.rs`, `collect_changes`, serializes each changed component's current value,
stamped with the frame's last tick. A 16-tick frame therefore sends one state, the last: the 15
states before it were never sent. Append-only diffs (the construction ledger) lose nothing.

**What a player saw.** Lightyear holds a sample past the newest keyframe rather than extrapolating
(`lightyear_interpolation` `registry.rs`, `sample_history_with_interpolation`). The shipped buffer
is `spread + tick` beyond RTT compensation, measured earlier at 22 to 32 ms of `min_delay`
(`net-motion-contract.md`), so a 387 ms gap held every remote hull and fallen piece still for
about 350 ms, then stepped it 16 ticks at once: 2.5 m for a hull at 10 m/s, about 2.6 such steps a
second for the whole fall. The arrival estimator cannot absorb this in time, because `interp_delay`
acquires coverage at 0.1 s per second and the gaps call for more than the fall lasts.

DERIVED by replaying the client's laws (`RemoteEstimate`, the sync controller, `interp_delay`
pacing, the arrival estimator, the sample clamp) over run 1's measured send times, for a hull at
10 m/s across the fall and the following 400 ticks, at 60 Hz and 40 ms RTT/2. The script is the
lane's, in its scratchpad; it is a model of those laws, not a capture.

| Send cadence during catch-up | Time frozen | Median / longest freeze | Steps over 0.5 m | Backward hops, largest |
|---|---:|---:|---:|---:|
| Once per frame (before) | 6.2 s | 233 / 517 ms | 22 | 3, 1.6 m |
| Every tick (chosen) | 0.9 s | 17 / 117 ms | 1 | 10, 1.5 m |
| Every 2 ticks | 1.6 s | 33 / 133 ms | 1 | 8, 1.6 m |

**Decision.** Every frame runs one tick, so every tick is sent, the cadence a tick under budget
already has. No new number exists to tune. A cadence of two ticks halves the send work and
leaves a 33 ms hold on each send through the fall.

**Cost.** MEASURED hosted: a send costs 0.7 to 1.7 ms during the fall, mean 1.1 ms. DERIVED: 199
sends cost about 0.22 s across the 5.15 s fall (14 sends cost 0.015 s), so the simulation's speed
falls by about 4 %, from 0.60 to roughly 0.58. The rest of each frame's work is not priced here.

**Still visible.** While the simulation runs at 0.6 of wall speed the client's cursor, bounded to
about 0.81 to 1.2 of wall speed, cannot follow it. It drifts ahead of the stream until the
controller's 10-tick margin snaps it back: the backward hops above, each about 1.5 m for a hull at
10 m/s. Send cadence does not cause the drift, but a denser stream lets the remote estimate follow
the slowed clock sooner, so hops that a held view used to hide now show. MEASURED, local with other
lanes compiling, three scheduled collapses: the one client logged 28 `FRONTIER interpolation resync`
warnings before this change, and 55 and 80 on two runs after it, which simulated 2,253 and 1,862
ticks in 45 s against 2,431 before. Machine load differed between the runs, so they price nothing. Only lower tick cost removes them, or a client cursor
allowed to follow a slowed stream. Raising the margin alone did not help in the replay: at 25 ticks
the hops fell to 4 and grew to 3.8 m.

## Supplied evidence

MEASURED in `stall-evidence/baseline/server.log` from the lane's external scratchpad:

- The last stats line reports a 650.842 ms maximum tick and 1,924 construction
  pieces after the first event. No subsequent stats line appears before timeout.
- Later fixed-loop events print `actual_s` 26.813 and 46.269.

Those events establish fixed-loop progress, not simulation speed. `src/net/loadtest.rs`,
`drive`, computes `actual_s` from `std::time::Instant::elapsed`, independently of
Bevy's clocks. The proposed 57% simulation-speed inference is therefore unsupported.

## Reproduction

The original command was rerun with temporary `First`, `FixedFirst`, `FixedLast`
and `Last` markers and stack samples at diagnostic wall offsets. All runs were
headless, silent, local, and held the machine lock. This is an unpriced diagnosis
on a host reporting ten available cores, not a hosted capacity result.

```sh
scripts/perf/collapse.sh --target local --rehearsal --tanks 6 --schedule scripts/perf/match-collapse.json
```

MEASURED: it failed again with `server did not finish the observation by the wall
deadline` (the runner's default 240 s watchdog). The last stats line was at workload
wall time 10.674 s, with a maximum tick of 654.712 ms and 1,924 construction pieces.

MEASURED markers from `target/server-stall/rehearsal/server.log`:

```text
frame=2651 first       wall=16.156359 delta=250ms cap=250ms
frame=2651 fixed_first wall=16.158519 sim=13.921875 overstep=237.676542ms
frame=2651 fixed_last  wall=32.947269
frame=2651 fixed_first wall=32.947386 sim=13.937500 overstep=222.051542ms
frame=2651 fixed_last  wall=54.208070
frame=2651 fixed_first wall=54.208134 sim=13.953125 overstep=206.426542ms
frame=2651 fixed_last  wall=97.570558
frame=2651 fixed_first wall=97.570702 sim=13.968750 overstep=190.801542ms
```

DERIVED from those measured boundaries: the first three tick brackets in this frame
lasted 16.789 s, 21.261 s and 43.362 s. The fourth never closed before timeout. There
was no later `First` or `Last`. The accumulator decreases by the configured tick
without being replenished; this frame has not exhausted its finite batch.

MEASURED samples in `target/server-stall/rehearsal/sample-45.txt` and
`sample-100.txt` show the main thread inside `run_fixed_main_schedule`, waiting on
its nested fixed schedule, and working compute threads inside `weigh`. The first
sample includes `force_wrench`/group membership searches; the second includes the
same law assessment and graph walks. This distinguishes expensive game work from
a packet, log or frame-system block.

The timing witness uses the same `ServerPlugins { tick_duration: TICK }` with
`MinimalPlugins` and `StatesPlugin`, manually admits a one-second wall delta, and
injects `sleep(TICK * 2)` in `FixedUpdate`. Its diagnostic 250 ms wall threshold is
fixture data, not an approved service contract. The measured default-cap result:

```text
MEASURED probe: frame work gap=639.648875ms, virtual cap=250ms, fixed elapsed=250ms
frame work waited 639.648875ms while fixed ticks were over budget
1 failed; finished in 0.71s
```

Command (the queue holds the machine lock through the test run):

```sh
export OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
scripts/serial-cargo test --lib --features dev_ui,bitprobe server_stall_probe -- --nocapture
```
The temporary probe is preserved in the bay's `target/server-stall/probe.red.rs`;
its module was mounted under `net::server` for the measurement. No fixture threshold
will become a production regression obligation without an owner-chosen contract.
The same diagnostic passed after changing only the virtual cap to `TICK`:

```text
MEASURED probe: frame work gap=40.177459ms, virtual cap=15.625ms, fixed elapsed=15.625ms
1 passed; finished in 0.06s
```

The red/green comparison proves the catch-up multiplier for bounded injected work.
It does not establish a service guarantee for the real construction workload.

To reproduce the diagnostic in a clean bay, temporarily place this test module in
`src/net/server.rs` and use the command above (add the cap line only for the
candidate control). All numbers labelled Fixture belong to the test, not the
production scheduler:

```rust
#[cfg(test)]
mod server_stall_probe {
    use super::*;
    use bevy::time::TimeUpdateStrategy;

    #[test]
    fn frame_work_is_not_starved_by_over_budget_ticks() {
        // Fixture: a tick sleeps for twice its budget; a frame receives a full second.
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .add_plugins(ServerPlugins { tick_duration: TICK })
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1)))
            .add_systems(FixedUpdate, || std::thread::sleep(TICK * 2));
        app.finish();
        app.cleanup();
        app.update();
        let start = std::time::Instant::now();
        app.update();
        // Fixture: diagnostic service interval, not an owner-approved requirement.
        assert!(start.elapsed() < Duration::from_millis(250));
    }
}
```

The single candidate-control change, before `app.finish()`, is
`app.world_mut().resource_mut::<Time<Virtual>>().set_max_delta(TICK);`.
Neither arm is retained in compiled code.

## Outcome requiring an owner decision

The library default already bounds admitted simulation time. A tighter bound
changes how much wall time is discarded and what clients observe, so it requires
a pacing decision. The clamp discards elapsed wall time, not simulation
tick IDs; tick IDs remain consecutive. A finite admitted delta also does not bound
cumulative wall-to-simulation clock divergence under sustained overload.

The required outcome is explicit overload handling: receive, send and stats keep
running, and any departure from real-time simulation is visible. The implementation
must expose an overload state/cause at the frame boundary rather than silently
inventing elapsed time. An invariant promised for a supported workload that the
implementation breaks is a bug; an operator/OS capacity limit is outside input and
needs a typed, visible outcome. Neither classification authorizes an unannounced
change to game time.

| Option | Benefit | Cost / policy the owner must choose |
|---|---|---|
| Retain the default and reduce expensive tick work | Preserves the existing time policy | Does not itself promise a packet-service deadline; establish a supported workload and bound the complete tick |
| Lower `Time<Virtual>::max_delta`, potentially to `TICK` | Uses Bevy's mechanism; a one-tick cap removes the catch-up multiplier | Discards wall time sooner, slowing simulation under lighter overload; assess client clock steering, input age and timeouts; still blocked for one whole tick |
| Yield after bounded work while retaining backlog | Can service frame work without deliberately discarding elapsed time | Backlog and player latency grow under sustained overload; requires an explicit backlog/admission policy and changes beyond the library's existing loop |
| Refuse excess work or end an overloaded Battle visibly | Makes capacity failure an explicit outcome | Needs an owner-approved admission/refusal/end policy; cannot silently omit existing game state |

Before implementing a wall-service regression test, choose a service interval and
its premise: a maximum complete-tick cost under a declared workload. The test must
mount the production scheduler and demonstrate `PreUpdate`, `PostUpdate` and `Last`
progress under over-budget ticks. A threshold chosen merely to bless a measured run
would not be a contract. Also choose the maximum permitted clock divergence or
backlog and the visible outcome when it is exceeded.

## Idle CPU observation

MEASURED in the supplied `hosted-202b/server-final.log`: 74 steady idle windows after
startup, zero tanks and clients, total reported span 74.0 s. Weighted process CPU is
45.13% of one core. Fixed-tick wall brackets total 8.995% of one wall-time core.

DERIVED: the difference is 36.14 percentage points. This is not a measurement of
frame-loop CPU alone: one quantity sums process thread CPU and the other brackets
fixed-schedule wall time. A controlled loop-period comparison is needed for that
attribution. The controlled comparisons below measure whole-process CPU instead.

A controlled local comparison used the same binary and idle world, changing only
the runner period through a temporary startup control. Probe systems were absent
in these idle runs. The order was 4 ms, 15.625 ms, 15.625 ms, 4 ms, with 15 steady
stats windows per run after excluding startup. MEASURED in
`target/server-stall/idle/`:

| Runner period | Mean process CPU, % of one core |
|---|---:|
| 4 ms, first run | 40.54 |
| 15.625 ms, first run | 22.55 |
| 15.625 ms, second run | 22.06 |
| 4 ms, second run | 41.83 |

MEASURED: every sampled window had zero clients, zero tanks, 5,961 entities and no
tick overruns. DERIVED from the per-window CPU readings: the two 4 ms runs average
41.19%; the two tick-period runs average 22.30%; the incremental cost of the shorter
period is 18.88 percentage points of one core on this local host. This compares
whole idle processes, rather than subtracting tick wall time from process CPU.
It does not measure hosted savings or the cost of all frame work, which still runs
at the longer period. That diagnosis removed its startup control without changing
production pacing; #278's tick-paced follow-up is measured below.

## Stall diagnostic verification

The stall diagnosis restored the original scheduler source and removed its test
arms, frame markers and startup controls without changing the library default or
construction laws. All processes started by those diagnostic runners were
terminated and reaped. The tick-paced follow-up changes only the pacing described
above, preserving that overload policy.

The stall diagnosis reproduced the failing rehearsal above. It performed no
post-fix rehearsal and added no production wall-service regression test because
no pacing/capacity contract had been selected. The source and timing witnesses
remain the basis for a future regression test once that contract is chosen.

Passed: `cargo fmt --all --check`, `git diff --check`, strict locked workspace
clippy on all targets with `dev_ui,bitprobe`, and the targeted server-stats tests
plus `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards (54 passed).
The locked normal server/client build with the same features also passed, replacing
the temporary instrumented binaries. The full suite was not run; it belongs to CI.

## Tick-paced local measurements

MEASURED on Apple M4, macOS 26.5.2, ten available cores. Every run was local,
headless, unattended and muted, under the machine lock. One temporary binary
selected the former runner or the tick-paced runner at startup; the world and
other code were identical. The temporary control and probes are removed. No hosted
capacity or savings claim follows from these local observations.

Idle order: old/new/new/old, 21 wall seconds each, retaining the last 15 complete
one-second stats windows per run. Every retained window had zero clients/tanks,
5,962 entities and no tick overruns. CPU is process user+system time as a percentage
of one core, weighted by each stats window's reported wall span.

Tick lateness brackets the due time through entry to `FixedFirst`. For each tick,
its due time is the frame's real-time sample minus the remaining fixed overstep
(after that tick was expended). This is the tick's accumulator boundary, not a
second periodic clock. Lateness and frame counters cover the last 15 wall seconds;
their separate schedule brackets can differ by one boundary sample. Quantiles use
nearest rank over raw samples.

| Idle run | CPU, % of one core | Lateness p50 / p99, ms | Frames with zero / two / more ticks |
|---|---:|---:|---:|
| old, first | 49.15 | 2.760 / 5.164 | 2,270 / 0 / 0 |
| new, first | 24.64 | 3.077 / 3.878 | 0 / 0 / 0 |
| new, second | 23.96 | 3.104 / 3.933 | 0 / 0 / 0 |
| old, second | 43.45 | 2.862 / 5.146 | 2,250 / 0 / 0 |

DERIVED from those equally sized windows: old/new mean CPU is 46.30% / 24.30%
of one core. Pooled raw idle lateness p50/p99 is 2.820/5.146 ms old and
3.093/3.885 ms new: median lateness increased while the upper tail decreased.
MEASURED: the paired idle runs counted 4,520 tickless old frames and zero new;
neither runner had a multi-tick steady idle frame.

One local headless client ran the same Idle input script in a separate old/new
trial, 36 wall seconds each. The last 15 server stats windows had one tank, one
client, 6,135 entities and no tick overruns. A temporary `MessageReader` timestamped
each `MutateTickReceived` after replication receive. These are update application
arrival times at the client frame, including its drain/replication work, not raw
socket arrival times. Each trial retained 960 updates over its final 15 seconds.

| Connected observation | old | new |
|---|---:|---:|
| Server CPU, % of one core | 32.18 | 21.83 |
| Tick lateness p50 / p99, ms | 2.669 / 5.060 | 2.948 / 3.672 |
| Frames with zero / two / more ticks | 2,226 / 0 / 0 | 0 / 0 / 0 |
| Update interval p1 / p50 / p99, ms | 11.333 / 14.097 / 20.797 | 11.287 / 15.745 / 20.210 |
| Update interval standard deviation, ms | 2.443 | 1.689 |
| Update interval p99 minus p1 spread, ms | 9.463 | 8.923 |

Raw logs, measurement summaries, the temporary probe source and harness, and the
controlled-clock red/green outputs remain locally under `target/tick-clock/`.
All spawned server/client processes were terminated and reaped. The orchestrator
owns the two-core hosted measurement and the separate overload policy decision.


## Tick-paced verification

Passed: `cargo fmt --all --check`, `git diff --check`, locked strict workspace
clippy on all targets with `dev_ui,bitprobe`, and targeted Rust tests (458 passed,
seven ignored) covering `net::`, `cost::`, `trace::`, `avian_child_pose_churn`,
`ballistics::motion`, `doc_citations`, `ui_ascii`, `net_boundary` and `bake::`.
The existing `test_online_cost` and `test_collapse` parser/runner suites also passed
(28 tests, mocked processes). Locked normal server/client binaries were rebuilt
with the same features after removing instrumentation. The full suite belongs to CI.
