# Shot runner #386

Subject: the multiplayer tank game's shot harness and damage model; game state only.
Rounds are spawned, traced and reported; modules and crew are HP pools.

Base: `a1e02ccdb93c243dc916b66f7c81649edc328ae6`, in leased bay 3 on `lane/shot-runner`.

## Cause and baseline evidence

MEASURED, one original MG run on this tree, with mute supplied by the caller:

```
OVERMATCH_MUTE=1 OUT=/tmp/shot-runner-before-1 scripts/shot/run-mg-armor.sh
client_trace_ended_early_by_ticks=86, lost_shots=4, no_spawn_shots=2, public_path_failures=4
```

The cause is the capture's input/observation boundary. At the base revision,
`src/net/harness.rs:613` exits when the script has authored its last input, while
`SimulateInput::write` still holds the secondary trigger through that input.
`scripts/shot/run-mg-armor.sh:93` supplies no neutral observation phase and line 72 keeps only
the target alive longer. Authored input completion does not imply server execution, downlink
arrival, or crossing the shared presentation cursor.

MEASURED from that capture:

- Both clients completed normally; neither watchdog fired. The server recorded exactly two
  connects and no disconnect during the capture.
- The server emitted 170 fires; the shooter received 166 and spawned 164. The target received
  and spawned all 170. Every missing shooter's fire had three accepted visual sends to two
  recipients; all four paths also had accepted reliable sends. There were no send errors,
  expired visual facts, or recipient omissions.
- The missing pairs were `(combatant=2, weapon=0/1, fire_tick=1160/1166)`. The first pair was
  emitted at wall time `1794863704810593` us; the shooter completed at
  `1794863704854464` us. Its configured receive latency was 80 ms with 10 ms jitter.
  The second pair was emitted after that completion, from the input already filed.
- The received-but-unspawned pair fired at tick 1154. The shooter's final receive row reports
  local tick 1163 and presentation cursor 1151.036: the cursor had not released those shots.
- The original aim produced only ground outcomes and zero damage confirmations. The old
  analyzer also inferred ownership from `spawn src="own"`, although every reconstructed
  client spawn in this capture was `src="obs"`. Private-damage accounting was therefore empty.

DERIVED: the four missing fire/path pairs are capture truncation, not a demonstrated transport
loss. The two missing spawns are pending cursor release, not missing replica construction.
The trace-end gap also includes the target-only tail and event sparsity. A real capture-end
row is needed to establish coverage during quiet observation.

## History checked

Read diffs instead of rebuilding old commits. `105ad2a1` added the shared slot barrier but
retained the target-only tail. `d969ae8f` changed release-relative script timing and retained
immediate exit at input completion. `83380400` introduced prediction/retiming classification;
that identity rule predates ADR-0037's fused server-fire presentation. `e0314602` installed
public timed-path playback and reliable paths while retaining that old identity classification.
`fdf165c9` adapted fan-out to lightyear 0.31's authority endpoint. `476341ae` changed grounding
to construction-derived stance. None of these readings establishes a last passing MG run;
this investigation does not attribute the MG failure to the engine upgrade.

## Fix

The harness accepts an exclusive `fire_until` within a Fire script. Inputs after it keep aiming
and release both triggers. The MG runner uses its existing capture budget for a neutral
observation tail and gives both clients the same script length. The watchdog is unchanged.
The harness records control identity independently of shot receipts and records capture
completion explicitly. The analyzer scores own and remote server fires by the same exact
identity; prediction, retiming and own-echo tail exemptions are removed. The existing
32-tick endpoint guard and all missing delivery/spawn/path/marker checks remain strict.
The MG runner additionally requires a completed capture and nonempty shooter damage evidence.

All shot runners set unattended, hidden and mute mode. No wire type changed: `Script` and
`SimulateInput` are absent from `WIRE_TYPE_DEFS`; protocol revision and wire hashes are unchanged.

## Other runners

MEASURED comparison runs, with only unattended/hidden/mute launch corrections:

| Runner | Runs | Result | Evidence |
|---|---:|---|---|
| `run-pencam-latency.sh` defaults | 1 | PASS | 2 connects, 5,082 MG fires, 51 main-gun fires; one explanation per client, no sent explanation missing |
| `run-combat-start.py` | 1 | FAIL | No authority fire; initial tick-zero input for tick 336 arrived at server tick 340, after simulation |
| `run-walk.py` defaults | 2 captures | PASS | 170.104 / 170.001 live seconds; each had 30 tanks, 709 constructions, 2 construction crossings, 26 construction changes, 0 unresolved-contact stops and 0 panics |

These runners have different gates. Pen-cam defaults inject no latency/jitter; walk has no
client. The original MG target's complete receipt stream is the direct transport control under
the MG profile. Combat-start exposes a separate first-input delivery defect and remains open;
no retry or deferred first click was added to conceal it.

## Validation

MEASURED: CI workspace/all-target clippy passed with `dev_ui,bitprobe`; all 17 targeted
`net::harness` tests passed. Python tests: 20 analyzer, 3 runner/process, 9 pen-cam tests passed.
The new identity/receipt regression failed before the analyzer fix. The original captured
trace still produces its original four strict failures with the revised analyzer. Conversely,
the original analyzer refuses final run 1 as `stray_shooter_markers=16`, independently proving
that its predicted-spawn ownership rule is stale.

The pose probe (`/tmp/shot-runner-pose-probe`) confirmed the trigger-tail fix independently:
MEASURED, 128/128 fires delivered and spawned, zero missing public paths, with only the newly
added `no_shooter_damage` gate refusing the run. Its idle-powered turret traversed at about
5.75 degrees/s and reached only 52.1 degrees at the final fire (tick 1024). An upper-side aim
requiring more traverse therefore still missed. The final MG fixture starts both hulls at a
45-degree turn and holds the target's exposed barrel down. Its aim needs about 33 degrees of
local traverse (DERIVED from `atan(6.9 / 10.6)`) and allows both MG streams to resolve within
the capture. This is fixture data, with a generic nonempty-HP-evidence gate; no tank-specific
logic, material, HP pool size, or game law changed.

One intermediate candidate used SIGINT to stop the zsh-background server. That signal was
ignored; after both clients had completed, the owned server was terminated with SIGTERM.
The retained runner uses its original SIGTERM shutdown. The intermediate candidate and pose
probe are diagnostic attempts, not passing after-runs; no automatic retry was added.

MEASURED final after-runs, all using the final fixture, the default 700-tick capture and the
original 80/10 ms conditioner. The current binary had already been built through serial-cargo.
One `/usr/bin/lockf` held `/tmp/overmatch-cargo.lock` around a Python driver that called
`scripts/shot/run-mg-armor.sh --holding-lock` once per distinct output directory. It stopped on
any nonzero result; it never retried a run.

| Run / output under `/tmp/` | Exit | Fires delivered / emitted | Public paths delivered / completed | Damage expected / delivered / marked | Lost / no-spawn / duplicate-spawn | Endpoint gap, ticks |
|---|---:|---:|---:|---:|---:|---:|
| `shot-runner-after-1` | 0 | 128 / 128 | 166 / 166 | 16 / 16 / 16 | 0 / 0 / 0 | -38 |
| `shot-runner-after-2` | 0 | 128 / 128 | 165 / 165 | 16 / 16 / 16 | 0 / 0 / 0 | -36 |
| `shot-runner-after-3` | 0 | 128 / 128 | 165 / 165 | 16 / 16 / 16 | 0 / 0 / 0 | -33 |

All three had completed captures, no missing/refused public paths, no missing/duplicate/stray
markers and no missing damage receipts. Transport recorded zero send errors, visual expiries,
public recipient omissions and private recipient omissions in every run. Counts are observations, not future test targets.
The endpoint gap is server minus client; negative values include the client's quiet tail.
Raw final summaries live in each output's `summary.json`; the driver log is
`/tmp/shot-runner-after.log`.

Open: combat-start's late first input, outside this MG capture fix. The last passing MG revision
remains unknown. The orchestrator still owns independent six-lens review, merge and push.
