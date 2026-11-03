# Battle verification

Subject: the game's Battle flow and damage-model knockout credit, game state only.

**Passed: results and the next fill-up are visible, correct and crash-free in this recipe.**
Both processes remained alive for at least MEASURED 75 wall seconds after the next fill-up
began, then were deliberately terminated. **Issue #243 can close.** Only this report changes;
no source, mode, balance, gate or runtime lever changed.

## Recipe and repeat

Lane `battle-verify-3`, bay-1, on `ui: source matchmaking styles from required overlay data`,
including `net: keep client input delay fixed across RTT saturation (#243)`.
MEASURED: both binaries built once through `scripts/serial-cargo`, with `dev_ui,bitprobe`.
Each run held one machine lock: one loopback authority, one hidden, silent, unattended client,
Kalinovo, the shipped `assets/modes/battle.ron`, and 30 fixture bots. Fixture video settings
remained cap 30 and debug panel off. No frame/tick cost recorder was armed.

The copied runner under `target/battle-verify/` stops after 75 seconds of the next fill-up
instead of the inherited 12-second stop; its watchdog remains 2700 seconds. The copied capture
plan's horizon was extended to that watchdog. These are fixture bounds, not game settings.
MEASURED: the first run completed from 19:59:25.342175 to 20:16:59.632983 UTC on 2026-11-03,
without a panic. Its inherited app-second 900-to-1000 capture gap missed results, which lasted
from 20:15:28.759005 to 20:15:43.760477. Its next-fill-up still shows `BATTLE STARTS IN 0:36`.

Decision: complete that run, then repeat with only five-second still cues added across the gap.
The repeat below supplies the complete visual evidence; its artifacts live in
`target/battle-verify/repeat/`. MEASURED repeat start: 20:21:01.521184 UTC; cleanup complete:
20:38:35.869085 UTC, 75.936 wall seconds after the next fill-up's authority announcement.
The runner observed both processes alive for at least 75 wall seconds after that announcement.
Both processes exited by deliberate SIGTERM, status -15; neither panicked or observably stalled.
All UTC times in this report are MEASURED on 2026-11-03.

## Results and next fill-up, MEASURED

| Phase | UTC | Tick | `repeat/server.log` line |
|---|---|---:|---:|
| Timed fill-up | 20:21:05.147918 | 176 | 78 |
| Combat | 20:22:04.948927 | 4016 | 79 |
| Late join closed | 20:32:04.929148 | 42416 | 451 |
| Results, Winner(CombatantId(20)) | 20:37:04.929325 | 61616 | 552 |
| Next fill-up | 20:37:19.933077 | 62576 | 553 |
| Next combat | 20:38:20.087232 | 66416 | 621 |

Personally inspected the retained stills:

- [Results table](../../../target/battle-verify/repeat/cycle-00965.0000s.png): `RESULTS 0:11`,
  `BATTLE 1 — WINNER: PLAYER 20`, with PLAYER 20 marked `WINNER`.
- [Next fill-up clock](../../../target/battle-verify/repeat/cycle-00980.0000s.png):
  `BATTLE STARTS IN 0:56`, the results table gone and fresh tanks present.

MEASURED: all 31 results rows are readable. I transcribed every row and compared it with
scores independently accumulated from the authority's 113 score facts in `repeat/analysis.json`:
every knockout count, loss count, order and shared rank agrees. PLAYER 20 has 11 knockouts and
seven losses; PLAYER 18 also has 11 knockouts but eight losses. The authority's unique winner
and the screen agree. The fill-up clock agrees with the announced phase and its 60-second
span; the client survives through the following combat release. No draw occurred.

## Whole-Battle audit, MEASURED

The copied `audit.py` passed unchanged assertions against each complete run and current data
owners. All knockout facts in each run belong to its completed first Battle; there were no
additional knockout facts during the observed opening of its second combat.

| Check | Repeat with results still | First run |
|---|---:|---:|
| Independent marker transitions | 113 | 139 |
| Score/loss entries | 113 | 139 |
| Shooter credits | 113 | 139 |
| Marker and score keys | Equal | Equal |
| Repeated combatant/life marker keys | 0 | 0 |
| Completed combat respawns | 113 | 136 |
| Respawn wait range, authority ticks | 640–644 | 640–642 |
| Server / client panics | 0 / 0 | 0 / 0 |
| Client interpolation-resync warning lines | 4 | 6 |
| Results / next-fill-up authority phases | Both | Both |

DERIVED required wait: 640 ticks, from the shipped ten-second wait and `server::TICK`.
MEASURED repeat waits: 640, 642 and 644 ticks. Every observed respawn followed its prior-life
knockout and met the required wait. Marker/score equality includes Battle, combatant, life,
entity and tick. Full outputs are `audit.json` and `repeat/audit.json`.

MEASURED repeat diagnostics: four authority `server_late_input_mismatch` ERROR lines
(`repeat/server.log` lines 80, 268, 617 and 622), no client ERROR lines, and 59 client cosmetic
path-capacity warnings. The first run has seven authority ERROR lines, no client ERROR lines,
and 74 cosmetic path-capacity warnings. These observations remain in the logs; no check was
loosened and no correction, warning or resync was suppressed.

## Authority clock against wall clock: Mac reading

MEASURED host: Apple M4, 10 available cores, compute/async-compute/IO pools 10/1/1. This is a
**Mac reading, not a two-core reading**. No two-core lever was used. This reads existing log
facts and timestamps; it measures no frame or tick cost and attributes no performance cause.

MEASURED origin: timed fill-up at tick 176, 20:21:05.147918 UTC. DERIVED nominal scale:
64 ticks per second from `server::TICK`. For each wall minute, the table takes the nearest
exact authority phase, spawn or score fact and shows its actual timestamp offset. It does not
interpolate to the minute. DERIVED largest sample offset from its requested minute: 15.007 s.
The older periodic diagnostic dumps were removed by `diagnostics: remove unread server and
client log dumps`; no recorder or additional logging lever was armed to replace them.

Authority seconds are DERIVED as `(tick - 176) / 64`; progress/wall is the authority-second
difference divided by the actual timestamp difference from the preceding row. Log lines are
in `target/battle-verify/repeat/server.log`.

| Wall minute | Actual wall s, MEASURED | Tick, MEASURED | Authority s, DERIVED | Progress / wall, DERIVED | Log line |
|---:|---:|---:|---:|---:|---:|
| 0 | 0.000 | 176 | 0.000 | — | 78 |
| 1 | 59.801 | 4016 | 60.000 | 1.0033 | 79 |
| 2 | 118.890 | 7796 | 119.062 | 0.9996 | 189 |
| 3 | 178.968 | 11639 | 179.109 | 0.9995 | 282 |
| 4 | 240.922 | 15606 | 241.094 | 1.0005 | 328 |
| 5 | 302.907 | 19572 | 303.062 | 0.9997 | 359 |
| 6 | 359.757 | 23211 | 359.922 | 1.0002 | 382 |
| 7 | 435.007 | 28027 | 435.172 | 1.0000 | 399 |
| 8 | 482.043 | 31039 | 482.234 | 1.0006 | 415 |
| 9 | 537.891 | 34614 | 538.094 | 1.0002 | 426 |
| 10 | 602.165 | 38727 | 602.359 | 0.9999 | 431 |
| 11 | 659.781 | 42416 | 660.000 | 1.0004 | 451 |
| 12 | 724.210 | 46533 | 724.328 | 0.9984 | 483 |
| 13 | 780.017 | 50110 | 780.219 | 1.0015 | 499 |
| 14 | 829.134 | 53253 | 829.328 | 0.9998 | 515 |
| 15 | 901.088 | 57858 | 901.281 | 1.0000 | 530 |
| 16 | 959.781 | 61616 | 960.000 | 1.0004 | 552 |
| 17 | 1034.939 | 66416 | 1035.000 | 0.9979 | 621 |

MEASURED phase intervals: combat's exactly DERIVED 900 authority seconds took 899.980 wall
seconds (DERIVED mean progress 1.00002); results' 15 authority seconds took 15.004 wall seconds.
The next fill-up's 60 authority seconds took 60.154 wall seconds. The first run's combat took
900.025 wall seconds and results took 15.001 wall seconds. Both runs' full minute tables and
source samples remain in their `clock-table.md` and `analysis.json` artifacts. No sustained
clock slowdown like the prior report's run is observed here; this is an observation, not a gate.

## Knockout mix, MEASURED

The game's damage model recorded **109 cook-offs and four crew-loss knockouts** in the repeat's
completed Battle. The first completed Battle recorded **135 cook-offs and four crew-loss
knockouts**. Reported as observed, with no tuning or explanation attached.

## Checks and disposition

MEASURED PASS: `cargo fmt --all --check`; CI clippy with `--locked --workspace --all-targets
--features dev_ui,bitprobe -- -D warnings`; and the targeted/guard run below, 79 passed,
none failed or ignored:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe -- \
  net::battle net::score input_buffer input_delay native_timeline_shift \
  doc_citations ui_ascii net_boundary bake::
```

Logs: `target/battle-verify/{build,fmt,clippy,tests}.log`. The binary build succeeded with the
linker's compact-unwind section-size warning. `git diff --check` passed. The report was checked
against all five lenses; no backlog row names it. No source fix was needed.

#243's results table, authoritative winner and next-fill-up clock are now observed and agree;
its hidden-window acceptance and this recipe's no-crash full-Battle gate pass. #243 can close.
All lane-started game processes are stopped. Logs, stills, copied runners and analyses remain
only under this bay's `target/battle-verify/`; only this report is committed. No merge, push,
issue comment or bay release occurred.
