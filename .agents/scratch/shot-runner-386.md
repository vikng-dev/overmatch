# Shot runner #386

Subject: the multiplayer tank game's shot harness and damage model; game state only.
Rounds are spawned, traced and reported; modules and crew are HP pools.

Base: `a1e02ccdb93c243dc916b66f7c81649edc328ae6`, in leased bay 3 on `lane/shot-runner`.

## Cause and baseline evidence

MEASURED, one original MG run on this tree:

```
OUT=/tmp/shot-runner-before-1 scripts/shot/run-mg-armor.sh
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
identity; prediction, retiming and own-echo tail exemptions are removed. The
32-tick endpoint guard and all missing delivery/spawn/path/marker checks remain strict.
The MG runner additionally requires a completed capture and nonempty shooter damage evidence.

Current runner modes: unattended throughout; MG and pen cam clients hidden, combat-start
headless, walk server with no window setting. No wire type changed: `Script` and
`SimulateInput` are absent from `WIRE_TYPE_DEFS`; protocol revision and wire hashes are unchanged.

## Other runners

MEASURED initial comparison runs. Combat-start used hidden mode here, so this is not
evidence for the GPU-less root; the headless follow-up below supersedes that verification:

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
the original analyzer refuses initial after-run 1 as `stray_shooter_markers=16`, independently proving
that its predicted-spawn ownership rule is stale.

The pose probe (`/tmp/shot-runner-pose-probe`) confirmed the trigger-tail fix independently:
MEASURED, 128/128 fires delivered and spawned, zero missing public paths, with only the newly
added `no_shooter_damage` gate refusing the run. Its idle-powered turret traversed at about
5.75 degrees/s and reached only 52.1 degrees at the final fire (tick 1024). An upper-side aim
requiring more traverse therefore still missed. The initial MG fixture starts both hulls at a
45-degree turn and holds the target's exposed barrel down. Its aim needs about 33 degrees of
local traverse (DERIVED from `atan(6.9 / 10.6)`) and allows both MG streams to resolve within
the capture. This is fixture data, with a generic nonempty-HP-evidence gate; no tank-specific
logic, material, HP pool size, or game law changed.

One intermediate candidate used SIGINT to stop the zsh-background server. That signal was
ignored; after both clients had completed, the owned server was terminated with SIGTERM.
The retained runner uses its original SIGTERM shutdown. The intermediate candidate and pose
probe are diagnostic attempts, not passing after-runs; no automatic retry was added.

MEASURED initial after-runs, all using the barrel-only fixture, the default 700-tick capture and the
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
Raw initial after-run summaries live in each output's `summary.json`; the driver log is
`/tmp/shot-runner-after.log`.

The last passing historical MG revision remains unknown. The orchestrator owns independent
six-lens review, merge and push.


## Review follow-up on 4cefb9e1

Subject: the game's shot harness and damage model, game state only. These captures are local
verification, not server performance measurements.

Decisions and changes:

1. Removed the retired mute lever from every `src/` and `scripts/` mention, including examples,
   other runners and startup tests. `OVERMATCH_UNATTENDED=1` owns unattended behavior.
2. Restored combat-start's GPU-less `headless` root, removed walk's unused window setting and
   the fake server's window assertion. No first-input scheduling fix is included.
3. Kept the fixed tail: a client does not observe the remote reliable outbox or the last server
   shot. A condition joining those to cursor completion needs cross-process settlement plumbing.
   `Resolution`: 128 ticks. MEASURED from the initial three captures: last shooter receipt
   22–47 ticks after the final trigger input; cursor lag at receipt 9.5–27.2 ticks. DERIVED
   conservative need `ceil(47 + 27.2) = 75`, leaving 53 ticks of margin. The watchdog bounds
   execution; strict delivery and playback checks refuse unfinished captures.
4. Completed captures now fail on every received path lacking `path_done`, state-only paths
   included. The regression covers both effect and state paths.
5. `TraceInputError` refuses conflicting controlled identities and malformed shot rows at load,
   naming file, line, field or conflicting IDs, and recapture as the fix. A server exit is accepted
   only after the runner requested SIGTERM and reaped its termination status; clean early exit
   and failure after the last input both fail fake-process tests.
6. The harness clock transitions to `Finished` before recording `capture_end`; repeated watchdog
   calls are tested. Each shooter and target recorded exactly one end row in every MG capture
   below (MEASURED).
7. Removed `--require-shooter-damage`; strict mode owns shooter identity, completion and nonempty
   HP-confirmation checks. The completion row also declares a Fire workload, so removing the
   controlled row cannot evade the identity check.
8. A `fire_until` window must contain a trigger demand. Tests require the error to name the
   field, and the full grammar test includes it.
9. Corrected held-trigger and warmup prose, labelled default script budgets, bound the cutoff
   once, deleted the stale KTX2 explanation and the redundant test comment. The endpoint
   allowance now names its independent-exit/shutdown overlap reason.
10. Kept the armour runner and fixture names. The aim traverses the exposed module and then
    reaches hull/turret plate. Passive `tank_strike` rows name the reached sim volume and its
    authored armour role; the runner separately refuses zero shooter plate contacts. No law,
    material or HP pool was changed.
11. Removed dead percentile helpers and NumPy, unified report output, and recorded the actual
    `fire.effects.smoke.is_some()` choice in each server fire row. Only smoke-declaring rounds
    require ribbon evidence. An explicit missing smoke field refuses old traces rather than
    inferring it from calibre. These are harness/trace fields only: no `WIRE_TYPE_DEFS` type or
    custom codec changed; `PROTOCOL_REV` and pins are unchanged.

MEASURED: a plate probe passed before the repeat set: 128/128 fires, 188/188 public paths,
77 plate contacts and 3/3/3 expected/delivered/marked HP confirmations. It is an extra probe,
not a replacement for any repeat below.

MEASURED repeat set, one machine lock around all six sequential captures, no retries. MG
clients used hidden windows, unattended mode and the original 80/10 ms conditioner; both
clients kept the default 700-tick length, firing stopped at 572. Output directories are under
`/tmp/`; each contains raw traces and process logs. Driver: `/tmp/shot-review-six.log` and
`/tmp/shot-review-six-results.json`.

| MG output | Exit | Fires delivered/emitted | Paths delivered/completed/pending | HP expected/delivered/marked | Plate contacts | Lost/no-spawn/multi-spawn |
|---|---:|---:|---:|---:|---:|---:|
| `shot-review-mg-1` | 1 | 128/128 | 187/185/2 | 3/3/3 | 76 | 0/2/0 |
| `shot-review-mg-2` | 0 | 128/128 | 190/190/0 | 3/3/3 | 79 | 0/0/0 |
| `shot-review-mg-3` | 0 | 128/128 | 195/195/0 | 4/4/4 | 83 | 0/0/0 |

MEASURED: every run reached hull and turret plate (`Hull_UFP_Upper`, `Hull_Top`,
`Turret_Side`; further plate names are in the raw rows). Every HP confirmation reached one
marker. No send errors, expired visual facts or recipient omissions were recorded.

Run 1 correctly refuses two mid-capture own-shot swallows, not tail truncation. For combatant
2, weapon slots 0 and 1, fire tick 681, the shooter received the fire and reliable paths but
recorded `drop res=owed_swallow` at local tick 697. Its capture ended at tick 1100. The target
spawned both keyed replicas and completed both paths. The shooter's log records belt-delta
recovery for both slots at reveal tick 681 before the swallows. Source: `spawn_reconstructed_fire`
in `src/net/client.rs` returns before keyed reconstruction when `try_swallow_owed` succeeds;
`recover_unannounced_rounds` in `src/net/fire_presentation.rs` constructs its replacement with `shot: None`.
DERIVED: that replacement cannot consume the keyed reliable path; more quiet tail cannot
complete a replica that was never constructed. This pre-existing presentation behavior remains
open; the harness does not exempt its missing keyed spawns or pending paths.

| Headless combat-start output | Exit | Client tick-zero click | First diagnosed input tick | Server receive tick | Late by ticks | Authority fires |
|---|---:|---:|---:|---:|---:|---:|
| `shot-review-combat-1` | 1 | 109 | 114 | 117 | 3 | 0 |
| `shot-review-combat-2` | 1 | 106 | 109 | 114 | 5 | 0 |
| `shot-review-combat-3` | 1 | 106 | 111 | 114 | 3 | 0 |

MEASURED: all headless clients completed 256 script ticks successfully; the runner then failed
because the authority emitted no fire. Each server log contains
`server_late_input_mismatch` / `server received initial input for an already-simulated tick`.
Run 2 explicitly contains `fire_primary: true, for_tick: 109` received at server tick 114;
the client logged its tick-zero click at tick 106. In runs 1 and 3 the first diagnosed buffers
start at ticks 114 and 111 with `fire_primary: false`; they do not show receipt of the client's
original tick-zero click. The table distinguishes client-edge ticks from received wire ticks
rather than assuming them identical. No input timing behavior was changed.

MEASURED validation: `cargo fmt --all --check` and CI workspace/all-target clippy with
`dev_ui,bitprobe` pass. Targeted Rust: harness 18, shot transport 15, trace 2, hosting 23
(6 subprocess fixtures intentionally ignored), server startup refusal 1; all pass. Python:
shot suites 39, collapse suite 42; all pass. The added analyzer regressions failed on the old
implementation (9 failures across missing-field cases, identity, pending playback and smoke).
Development binaries built through `serial-cargo`; macOS ld emitted its compact-unwind size
warning and both builds succeeded. Gate logs: `/tmp/shot-review-{rust2,transport,trace,hosting,
server,clippy,build}.log`, `/tmp/shot-review-python-final.log`, `/tmp/shot-review-collapse.log`.

Open: the observed own-shot swallow, headless combat-start's late/missing first click, and the
orchestrator's independent six-lens review. Neither failed capture is represented as passing.
