# Server frame starvation under overload

Subject: the video game's server scheduler during environment destruction; game state only.

Status: decided by the owner on 2026-11-05, until a revisit before the Steam release (#279).
- **Overload policy:** catch up, the library default (option 1 in the table below), with every
  over-budget tick visible.
- **Ticks over budget are bugs:** a tick over its 15.625 ms budget is a cost bug to fix at its
  source, never a load the game tolerates as part of play. That includes a building collapse.
- **One clock:** the tick is the server's only clock (#278); the 4 ms frame loop goes.

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

- `src/net/server.rs`, `run`, mounts `ScheduleRunnerPlugin::run_loop(FRAME)` and
  `ServerPlugins { tick_duration: TICK }`. `FRAME` is 4 ms; `TICK` is 15.625 ms.
- Bevy `bevy_app/src/schedule_runner.rs`, `ScheduleRunnerPlugin::build`, calls
  `app.update()` synchronously. It sleeps only for the remainder of the frame period.
  The period cannot interrupt work already running.
- Bevy `bevy_time/src/virt.rs`, `Time<Virtual>::default`, caps the wall delta admitted
  to virtual time at 250 ms. `advance_with_raw_delta` discards excess elapsed wall
  time; this is already slow simulation under sufficiently severe load.
- Bevy `bevy_time/src/fixed.rs`, `run_fixed_main_schedule`, adds that delta once and
  spends the accumulator until less than a timestep remains. It never adds the time
  spent running those ticks back into the same frame's accumulator.
- DERIVED: with unchanged timestep and unit virtual speed, the previous remainder
  is less than one tick, so this server runs at most 16 ticks per frame.
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
work resumes after the sum of at most 16 complete tick costs plus the frame's other
work. Even a one-tick-per-frame policy cannot service packets during that tick.

The `FRAME < TICK` assertion does not prove that each tick gets its own frame. That
holds only when accumulated elapsed time is less than two ticks. OS scheduling
jitter and expensive ticks can both create a catch-up batch.

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
attribution. `FRAME` remains the owner's latency/CPU lever.

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
at the longer period. The startup control is removed; no runner edit is needed.

## Verification

The original scheduler source is restored byte-for-byte. Both diagnostic test
arms, frame markers and startup controls are removed from `src/`; the production
`FRAME`, library default and construction laws remain unchanged. All processes
started by the diagnostic runners were terminated and reaped.

The original rehearsal still fails as described above. There is no post-fix
rehearsal or production wall-service regression test because no pacing/capacity
contract has been selected. The source and timing witnesses are recorded here so
the follow-up can turn that chosen contract into a regression test.

Passed: `cargo fmt --all --check`, `git diff --check`, strict locked workspace
clippy on all targets with `dev_ui,bitprobe`, and the targeted server-stats tests
plus `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards (54 passed).
The locked normal server/client build with the same features also passed, replacing
the temporary instrumented binaries. The full suite was not run; it belongs to CI.
