# Battle verification

Subject: the game's Battle flow and damage-model knockout credit, game state only.

**Results reached; the no-crash playtest gate remains open.** The authority named
PLAYER 19 the winner and announced the next fill-up. The client then panicked before a
next-fill-up still was saved. **Issue #243 cannot close.** No source, mode, balance, gate or
runtime lever changed in this lane.

## Full Battle, MEASURED

Lane `battle-verify-2`, bay-3, on the source step `scratch: the Battle verification's runner and
report (#243)`. One loopback authority and one hidden, silent, unattended client on Kalinovo,
with the shipped `assets/modes/battle.ron` and 30 fixture bots. The fixture video settings remained
cap 30, debug panel off. Both binaries built once through `scripts/serial-cargo`; the run held one
machine lock. No frame/tick cost recorder was armed. The prepared runner, audit and capture plan
were copied to `target/battle-verify/`, changing only their root/output paths.

MEASURED UTC times and authority ticks (`server.log`):

| Phase | UTC | Tick | Log line |
|---|---|---:|---:|
| Timed fill-up | 11:20:40.526539 | 306 | 141 |
| Combat | 11:21:40.322324 | 4146 | 1105 |
| Late join closed | 11:32:39.538728 | 42546 | 10851 |
| Results, Winner(CombatantId(19)) | 11:40:50.025188 | 61746 | 15668 |
| Next fill-up | 11:41:04.795311 | 62706 | 15677 |

All times above are MEASURED on 2026-11-03. The runner started at 11:20:34.759635 UTC and
finished cleanup at 11:41:07.266291 UTC. The server remained alive until deliberate SIGTERM;
the client exited 101 after one panic. Both logs advanced through the transition: no process
stall was observed, although authority-clock progress fell behind wall time.

Personally inspected stills, retained outside git under `target/battle-verify/`:

- [Initial fill-up](../../../target/battle-verify/cycle-00030.0000s.png): `BATTLE STARTS IN 0:31`.
- [Scored combat Tab board](../../../target/battle-verify/cycle-00150.0000s.png): `13:31`, all
  31 rows readable.
- [Results](../../../target/battle-verify/cycle-01205.0000s.png): `RESULTS 0:12`,
  `BATTLE 1 — WINNER: PLAYER 19`; winner row has 12 knockouts and five losses.
- [Last saved still](../../../target/battle-verify/cycle-01215.0000s.png): `RESULTS 0:02`, same table.

MEASURED: the results winner, every row's score, ordering and shared ranks agree with the
99 authority score facts, independently accumulated in `analysis.json`. No draw occurred.
The authority's next-fill-up log exists; **no next-fill-up clock still exists**. The last saved
still is still results. The earlier fixed-cue gap did not miss results in this run.

## Crash diagnosis

MEASURED in `client.log`, lines 43183–43195: `assert_input_buffer_ranges` panicked at
11:41:05.910600 UTC's following frame:

```text
input-buffer start Tick(73918) exceeds encoder end Tick(73838)
```

The input slot for the next tank was claimed at MEASURED 11:41:05.291228 UTC (line 42804).
DERIVED: the rejected interval is inverted by 80 ticks. The guard prevented the upstream encoder
from mislabelling future input; deleting it or clearing the buffer would conceal the failure.

Source-backed mechanism: `shipping_input_delay` calls Lightyear's `fixed_input_delay`, whose
implementation is only fixed while effective RTT plus jitter margin fits its prediction allowance.
In lightyear_sync 0.30.1, `InputDelayConfig::input_delay_ticks` returns effective RTT ticks minus
that allowance above the bound. `InputTimelineConfig::recompute_input_delay_on_local_timeline_shift`
can therefore shrink the delay, while lightyear_inputs 0.30.1's `receive_local_timeline_shift`
moves buffer starts, remote marks and staged-message ends only by the timeline delta. The encoder
end also includes the recomputed delay. A retained buffer can consequently start after that end.
`native_timeline_shift_preserves_the_input_buffer` covers a fixed delay; the cadence test
`a_resync_that_shrinks_the_input_delay_still_stages_the_next_tick` does not assert the encoder range.

MEASURED corroboration: the client logged input-sync floor-plus-coverage of 11169.87 ticks at
11:41:05.845882 UTC (line 43113), then 11140.04 at 11:41:05.910600 UTC (line 43180), immediately
before the panic. This is far outside the helper's DERIVED 103-tick fixed-delay region
(its 100-tick prediction allowance plus the shipped three-tick delay). The run did not trace
actual input-delay recomputations or timeline deltas, so their exact before/after values are
unmeasured. This is the identified unsafe delay-change mechanism, not a claim to have replayed
that exact transition in isolation.

Decision: retain the crash as a finding. A safe correction must establish an actually fixed delay
contract or preserve authored input across delay changes; no verified one-change correction was
established here. No source fix or rerun was made. The separate slowdown was read from logs only.

## Whole-Battle audit, MEASURED

`target/battle-verify/audit.py` passed against the complete authority log and current data owners;
`audit.json` retains its output:

| Check | Observation |
|---|---:|
| Independent marker transitions | 99 |
| Score/loss entries | 99 |
| Shooter credits | 99 |
| Marker and score keys | Equal, including Battle, combatant, life, entity and tick |
| Repeated combatant/life marker keys | 0 |
| Completed combat respawns | 99 |
| Respawn wait range | 640–655 authority ticks |
| Server panics | 0 |
| Client panics | 1 |
| Client interpolation-resync warning lines | 1013 |
| Results / next-fill-up authority phases | Both observed |

DERIVED required wait: 640 ticks, the shipped ten-second wait divided by the server's timestep,
rounded up. Every observed respawn followed its prior-life knockout and met that wait.
MEASURED distinct waits: 640, 641, 642, 643, 644, 645, 646, 648, 649, 650, 652, 653, 655 ticks.
MEASURED diagnostics: three server ERROR lines (one headless render-app message, two late-input
corrections); one client ERROR line from winit during panic teardown. The Rust panic was the
failure; there was no evidence of a preceding process stall.

## Authority clock against wall clock

All values below come from existing timestamped logs; no pricing instrumentation ran.
MEASURED origin: the timed fill-up at tick 306. DERIVED nominal scale: 64 ticks per second from
`server::TICK`. The table samples the nearest logged fact to each wall minute and gives its
actual MEASURED wall offset and log line. Direct phase/spawn/score ticks are exact; other tick
ranges are DERIVED from one diagnostics batch's logged `Loading { since, ready_tick }` windows:
maximum start through minimum deadline minus one. `protocol::publish_shot_clock` publishes the
same tick to both clocks, and `shooting::tick_weapon_gate` matures due deadlines before Update
diagnostics. The ranges expose uncertainty instead of interpolating quiet score-log gaps.

Authority seconds are DERIVED by subtracting the origin tick and dividing by the nominal scale.
The last column is the DERIVED interval progress ratio against the preceding row, with both
endpoint ranges propagated. Knockouts are MEASURED cumulative score-log entries at the sample.

| Wall minute | Wall sample s, MEASURED | Authority tick, exact or DERIVED bound | Authority s, DERIVED | Progress / wall, DERIVED | Knockouts, MEASURED | Server log line |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0.000 | 306 | 0.000 | — | 0 | 141 |
| 1 | 59.796 | 4146 | 60.000 | 1.003–1.003 | 0 | 1105 |
| 2 | 119.519 | 7927–7985 | 119.078–119.984 | 0.989–1.004 | 26 | 2157 |
| 3 | 180.030 | 11806–11890 | 179.688–181.000 | 0.987–1.023 | 47 | 3201 |
| 4 | 240.855 | 15607–15644 | 239.078–239.656 | 0.955–0.986 | 58 | 4173 |
| 5 | 300.252 | 19327–19384 | 297.203–298.094 | 0.969–0.994 | 65 | 5125 |
| 6 | 360.268 | 23163–23228 | 357.141–358.156 | 0.984–1.016 | 72 | 6087 |
| 7 | 420.449 | 26941–26960 | 416.172–416.469 | 0.964–0.986 | 75 | 6995 |
| 8 | 480.502 | 30461 | 471.172 | 0.911–0.916 | 80 | 7873 |
| 9 | 540.606 | 33671–33673 | 521.328–521.359 | 0.835–0.835 | 84 | 8663 |
| 10 | 599.221 | 36296–36358 | 562.344–563.312 | 0.699–0.716 | 87 | 9313 |
| 11 | 660.151 | 39682–39745 | 615.250–616.234 | 0.852–0.884 | 93 | 10135 |
| 12 | 719.030 | 42506–42548 | 659.375–660.031 | 0.733–0.761 | 95 | 10852 |
| 13 | 779.665 | 45484–45505 | 705.906–706.234 | 0.757–0.773 | 95 | 11588 |
| 14 | 840.981 | 48432–48466 | 751.969–752.500 | 0.746–0.760 | 98 | 12334 |
| 15 | 899.435 | 50050–50113 | 777.250–778.234 | 0.423–0.449 | 99 | 12756 |
| 16 | 958.688 | 51970–52033 | 807.250–808.234 | 0.490–0.523 | 99 | 13236 |
| 17 | 1020.520 | 54060–54068 | 839.906–840.031 | 0.512–0.530 | 99 | 13748 |
| 18 | 1081.104 | 56472–56530 | 877.594–878.500 | 0.620–0.637 | 99 | 14356 |
| 19 | 1140.831 | 59027–59060 | 917.516–918.031 | 0.653–0.677 | 99 | 14996 |
| 20 | 1200.535 | 61461–61522 | 955.547–956.500 | 0.628–0.653 | 99 | 15604 |

MEASURED/DERIVED end points: results at 1209.499 wall seconds and exactly 960 authority seconds;
next fill-up at 1224.269 wall seconds and exactly 975 authority seconds. Combat's 900 authority
seconds took MEASURED 1149.703 wall seconds, DERIVED mean progress 0.783 of real time. Results'
15 authority seconds took MEASURED 14.770 wall seconds.

The slowdown is gradual: the first full interval whose upper bound is below 1.0 is the sample
near wall minute four (0.955–0.986), with a clearer decline by minute eight (0.911–0.916).
The lowest completed minute interval is near minute fifteen (0.423–0.449). The late intervals
recover to approximately 0.63–0.68 before results. These are clock-progress ratios, not tick costs.

What grew alongside it, MEASURED: cumulative knockouts rose from 58 near minute four to 80 near
minute eight and 99 near minute fifteen, then stayed at 99 through results. Every combat
`SIM-EVIDENCE` tank census was 31 simulated of 31 observed; results logged zero, next fill-up 31.
The log supplies no independent live wreck, loose-debris or crater census. Cumulative cook-offs
are not a live wreck count. No debris/crater growth or causal cost attribution is claimed.
The continuing slowdown after the knockout count stopped growing is preserved in the table;
results' clock recovery coincides with the logged zero-tank census. A causal explanation of that
slowdown needs a separate investigation; this lane made no pricing run or performance change.

## Knockout mix, MEASURED

The game's damage model recorded **96 cook-offs and three crew-loss knockouts** over the full
Battle. This is an observation, with no tuning or explanation attached.

## Checks and disposition

MEASURED: formatting (`cargo fmt --all --check`) and CI clippy
(`scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`)
passed. The targeted/guard run passed 79 tests with no failures or ignored cases:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe -- \
  net::battle net::score input_buffer input_delay native_timeline_shift \
  doc_citations ui_ascii net_boundary bake::
```

This includes the Battle/HUD, score, native-input guard/delay tests and all required whole-tree
guards. Logs are `target/battle-verify/fmt.log`, `clippy.log` and `tests.log`. Both application
binaries built successfully; the linker warned about compact-unwind section size. The unchanged
source passes these tests despite the reproduced live transition crash; they do not clear that
playtest gate. `git diff --check` passed; the report was reviewed against all five lenses and no
backlog row names it.

#230's previously completed attribution acceptance remains supported by this full-Battle audit.
#243's results table and winner are now observed and agree with the authority. Its next-fill-up
view remains unobserved because of the client panic, so **#243 cannot close**, and the owner's
no-crash full-round playtest gate remains open.

All game processes started by this lane were terminated. Logs, stills, copied runners and
analysis outputs remain only in `target/battle-verify/` in bay-3; nothing heavy is committed.
No merge, push, issue comment or bay release occurred.
