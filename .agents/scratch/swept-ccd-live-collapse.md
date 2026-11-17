# Swept CCD: same-binary live collapse rehearsal

Subject: Overmatch's environment destruction model, game state only.

## Decision

**(i): no measurable collapse-tail effect in this live rehearsal.** Keep the gate. Its CCD
saving is clear; the physical-tail observations and non-CCD physics times overlap. The earlier
before/after difference belongs to revision confounding and run spread, not a demonstrated gate
penalty. These runs do not apportion that earlier difference among the other revisions' sleep,
warm-start and game changes. No tank-mediated separation was established, so no mechanism fix
or additional tank/contact experiment is justified.

This closes the live causal question left open by the
[controlled scene](swept-ccd-controlled-collapse.md) and
[earlier two-revision investigation](swept-ccd-collapse-diagnosis.md). Their measurements stand;
their unresolved-attribution conclusions are superseded by this comparison.

## Method

MEASURED: five runs per arm, interleaved **off-1, on-1, off-2, on-2, off-3, on-3, off-4,
on-4, off-5, on-5**. One release build on `89bd76d0`, changing only per-collider gate
participation through the same temporary environment switch, read once. The body gate,
supported-shape filter, contacts, sleep/warm-start code and game laws were identical.
Both binaries stayed byte-identical throughout all ten runs:

| Binary | SHA-256 |
|---|---|
| `overmatch` | `35937418e4ef3bf9b3c4bc5731336f308cbfff9368739f16351b7489b747c4d0` |
| `overmatch-server` | `c9d623962666dc13d146237591232773dd74efff4b949ab9d0621d653dd52bc3` |

Built with `scripts/serial-cargo build --release --locked --features dev_ui,bitprobe
--bin overmatch-server --bin overmatch`. The measurement driver held `scripts/build-lock`
through all runs and load waits; no compile overlapped a game run. Each run admitted only at
one-minute load ≤4. The full `scripts/perf/collapse.py` workload and round-4 arguments were:

```text
--schedule scripts/perf/match-collapse.json --tanks 30 --clients 1 --duration 60
--rehearsal --bin target/release --holding-lock
```

The temporary copy changed only its root path and removed its mute assignment. Every game
process used `OVERMATCH_UNATTENDED=1` and the tool's own `OVERMATCH_WINDOW=headless`;
`OVERMATCH_MUTE` was absent. The real client was GPU-less; its log confirmed headless mode.
This follows the current instruction's window mode, rather than round 4's hidden-GPU override.
The full recipe includes the preceding two-house and four-house events, all 30 tanks, and the
six-house event at wall time 30 s. No tank-free fallback was used.

DERIVED statistics use the same cohort and definitions as the earlier report:

- Cohort: `house_0007`–`house_0012`, resolved from ID-sorted map instance data.
- Final sleep: the final completed awake episode's sleep tick minus the piece's first birth,
  times its recorded timestep. A final-awake retired piece contributes no invented sleep.
  Quantiles use nearest rank. Arm medians below are medians of run summaries, not pooled pieces.
- Awake: reconstruct episodes at event tick +320/+640. DERIVED at 64 Hz: +5/+10 simulation s.
- Heavy: physics >5 ms from the event through instrument wall time 60 s; not just 600 ticks.
- Timers: sum the entire server's first 600 ticks starting with `events_fired == 3`.
  DERIVED duration: 600 × 0.015625 = 9.375 simulation s.
- Quiet: mean over individual ticks in preselected wall windows [0,10) and [50,60), pooled by
  tick count. Both windows had zero awake debris in every run; separate means are below.
  These are quiet with respect to environment destruction, with tanks still running.

## Per-run observations

Every table value is **MEASURED**, or **DERIVED** from the recorded ticks/episodes as defined
above. Times are rounded only for display. `Awake` is +5/+10 s; `sleep` is p50/p90.

| Run | Admitted load | Final sleep, s | Awake | Heavy ticks | Physics, 600 ticks, s | CCD, 600 ticks, s | Quiet physics, ms/tick |
|---|---:|---:|---:|---:|---:|---:|---:|
| off-1 | 3.915 | 2.969 / 7.547 | 38 / 0 | 418 | 4.177 | 0.539 | 1.311 |
| on-1 | 3.966 | 1.750 / 6.578 | 85 / 0 | 434 | 3.107 | 0.184 | 0.633 |
| off-2 | 3.407 | 3.875 / 6.609 | 113 / 0 | 409 | 3.637 | 0.398 | 0.943 |
| on-2 | 3.683 | 1.234 / 10.594 | 95 / 93 | 519 | 3.780 | 0.160 | 0.656 |
| off-3 | 3.544 | 1.422 / 7.094 | 86 / 0 | 448 | 3.414 | 0.401 | 0.919 |
| on-3 | 3.804 | 3.344 / 6.906 | 104 / 0 | 452 | 3.780 | 0.196 | 0.629 |
| off-4† | 3.706 | 4.750 / 10.656 | 120 / 81 | 694 | 5.885 | 0.596 | 1.436 |
| on-4 | 3.555 | 2.766 / 10.750 | 88 / 44 | 390 | 3.623 | 0.186 | 0.657 |
| off-5 | 3.759 | 3.828 / 6.125 | 70 / 0 | 357 | 3.944 | 0.464 | 0.914 |
| on-5 | 3.627 | 4.047 / 5.203 | 51 / 0 | 283 | 2.630 | 0.131 | 0.666 |

† Off-4 completed all population, link, combat, event and tick-continuity observations but the
runner exited 1: its real client covered 2,987 of 3,584 expected tick slots (DERIVED 83.34%, below
its 90% gate). The server ran at MEASURED 44.998 Hz, and host load was 12.003 immediately after
the run despite admission at 3.706. It is an **invalid fairness capture**, retained and flagged
rather than silently discarded or replaced. The other nine exited 0 and were valid; all links
were complete. Every run reported server over budget, so none establishes hosted capacity.
The requested five observations per arm are shown; the valid-only comparison below checks that
this invalid run does not decide the conclusion.

## Per-arm range and median

Each cell is **minimum–maximum; median**, over all five prescribed runs, including flagged off-4.

| Metric | Gate off | Gate on |
|---|---:|---:|
| Sleep p50, s | 1.422–4.750; **3.828** | 1.234–4.047; **2.766** |
| Sleep p90, s | 6.125–10.656; **7.094** | 5.203–10.750; **6.906** |
| Awake +5 s | 38–120; **86** | 51–104; **88** |
| Awake +10 s | 0–81; **0** | 0–93; **0** |
| Heavy ticks | 357–694; **418** | 283–519; **434** |
| Physics, 600 ticks, s | 3.414–5.885; **3.944** | 2.630–3.780; **3.623** |
| CCD, 600 ticks, s | 0.398–0.596; **0.464** | 0.131–0.196; **0.184** |
| Physics minus CCD, 600 ticks, s | 3.013–5.290; **3.481** | 2.499–3.621; **3.437** |
| Quiet physics mean, ms/tick | 0.914–1.436; **0.943** | 0.629–0.666; **0.656** |
| Quiet CCD mean, ms/tick | 0.385–0.537; **0.395** | 0.085–0.095; **0.093** |
| Quiet physics minus CCD mean, ms/tick | 0.529–0.931; **0.548** | 0.536–0.578; **0.561** |

Separate quiet-window physics means, **minimum–maximum; median**, ms/tick:

| Wall window | Gate off | Gate on |
|---|---:|---:|
| 0–10 s | 0.866–1.587; **0.913** | 0.556–0.604; **0.573** |
| 50–60 s | 0.915–1.299; **1.022** | 0.692–0.761; **0.710** |

Valid-only sensitivity, **minimum–maximum; median** (four off, five on):

| Metric | Gate off, excluding off-4 | Gate on |
|---|---:|---:|
| Sleep p50, s | 1.422–3.875; **3.398** | 1.234–4.047; **2.766** |
| Sleep p90, s | 6.125–7.547; **6.852** | 5.203–10.750; **6.906** |
| Heavy ticks | 357–448; **413.500** | 283–519; **434** |
| Physics minus CCD, 600 ticks, s | 3.013–3.639; **3.360** | 2.499–3.621; **3.437** |
| Quiet physics minus CCD mean, ms/tick | 0.529–0.774; **0.539** | 0.536–0.578; **0.561** |

## Why (i) holds

The gate's CCD times separate: even the slowest on run is cheaper than the fastest off run.
Beyond that saving, neither sleep quantile, awake count nor heavy-tick count separates. The
paired on-minus-off sleep-p50 differences change sign, and the on arm's median is lower, opposite
the earlier claimed gate delay. Both arms produce a long tail. The all-run physics-minus-CCD
medians are 3.481/3.437 s off/on; excluding off-4 gives 3.360/3.437 s with overlapping ranges.
Quiet physics-minus-CCD medians are 0.548/0.561 ms, or 0.539/0.561 ms on valid captures. The
clear quiet physics saving is therefore the CCD saving, not evidence of changed collapse rest.

The ten live processes do not replay one identical state. The schedule reads wall time
(`src/net/loadtest.rs`), and the real client's inputs arrive through its own process; event ticks
and piece partitions vary. Those variations are retained as observations. This sample shows
no reproducible physical separation to attribute to tanks; it does not prove every possible
state has identical TOI corrections. Together with the controlled scene's identical physical
traces, it refutes the proposed gate-caused damping/rest explanation for the earlier comparison.
No engine/contact/sleep change was made.

## Retained record and cleanup

The [compressed record](swept-ccd-live-collapse-record.json.gz) retains each run's admission and
binary hashes, capacity verdict, event log lines, target-window snapshots, full piece census and
sleep episodes, and a compact array of every tick's identity, elapsed time, timestep, event count,
physics/CCD time and awake count. Its field list and statistic definitions are in the header.
It is measurement evidence, not an experiment switch or executable probe.

The temporary gate and adapted runner were deleted. Shipping `vendor/`, `src/` and `scripts/`
are byte-identical to `89bd76d0`; only this lane's measurement records and follow-up links change.
No reset, stash, push, web access, external comment, sub-agent or target-directory deletion was
used. Raw run artifacts remain outside the tree in `/private/tmp/ccd6/`; build/run/gate logs are
`/private/tmp/ccd6-build.log`, `/private/tmp/ccd6-flow.log`, `/private/tmp/ccd6-gates.log`.
Independent six-lens landing review remains with the orchestrator.

## Verification

MEASURED on the restored shipping source, before the commit:

- `cargo fmt --all --check`: passed.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe --
  -D warnings`: passed.
- `scripts/serial-cargo test --lib --locked --features dev_ui,bitprobe tests::avian`:
  **110 passed, zero failed or ignored**, edit profile.
- Independent archive decoder: all ten per-run metrics and every archived per-arm range/median
  reproduced; all binary hashes/load admissions, contiguous 600-tick windows, nine validity
  verdicts and zero awake debris in quiet windows verified.
- Staged/unstaged whitespace checks and shipping-source equality to `89bd76d0`: passed.

MEASURED archive size: **1,250,925 bytes**; SHA-256:
`1ed654879bb93cc72794d4f1d84d4de1f6706f0ec4326f7a57f400e56bd1e2c1`.
