Subject: the video game's environment destruction and damage model, game state only.

The report is the narrative owner. These are diagnostic instructions and artifact locations.

`sleep-probes.patch` applies to the lane base, whose commit subject is
`diagnostics: report collapse server cost and remaining proof`. It includes instrumentation
and two independent prototype wake interventions; none is a shipping fix.

After applying it, compile/run only in a leased bay through the build queue:

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1
export OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1
DIAG_KIND=1000 DIAG_ACTION=force scripts/serial-cargo test --lib --features dev_ui,bitprobe diag_pieces_rest -- --nocapture --test-threads=1
DIAG_KIND=1000 DIAG_ACTION=force_sleep_vel scripts/serial-cargo test --lib --features dev_ui,bitprobe diag_pieces_rest -- --nocapture --test-threads=1
DIAG_KIND=1000 DIAG_ACTION=force_frozen_wake scripts/serial-cargo test --lib --features dev_ui,bitprobe diag_pieces_rest -- --nocapture --test-threads=1
DIAG_KIND=native DIAG_ACTION=force_frozen_wake scripts/serial-cargo test --lib --features dev_ui,bitprobe diag_pieces_rest -- --nocapture --test-threads=1
```

MEASURED configuration: the intervention is applied after 3,200 updates, then the probe observes
3,200 more at 64 Hz (DERIVED 50 simulated seconds per phase). `force` uses Avian `SleepBody`;
`force_sleep_vel` additionally zeros only the sleeping body's velocity used in narrow-phase
prediction; `force_frozen_wake` instead suppresses a contact wake when neither participant is
an awake non-static body. These changes do not remove bodies or change their mass/type.
The prototype does not establish automatic rest or a complete upstream contact-cache fix.

`client-he.patch` and `client-spread.patch` apply to release tag `v0.7.0-alpha.10`.
Use that tag's assets as well as its binary. The main-gun slot is fixture input, not a generic
system assumption. Their RON scripts are included. The spread trace records fresh authoritative
`fire_rx` arrivals; it confirms accepted shots, unlike local fire attempts.

All raw logs/binaries remain under `target/diag-pieces-rest/`; `summary.json` lists large logs.
`client-panic.txt` is the retained final portion of `remote-client-he2.log`.
The profiling deployment logs are `remote-he.log` and `remote-spread-final.log`, with matching
metrics and `stop-*.log` confirmations. Pull-attempt timestamps are `pull-times.log` and
`pull-spread-times.log`. Both deployments were stopped; no `om-` deployment was touched.

Regenerate host-stage tables from retained logs:

```sh
python3 .agents/scratch/diag-pieces-rest/evidence/analyze.py target/diag-pieces-rest
```

Local logs name their intervention; each `[DEBUG-rest] summary`, `wake` and `bodies` line is JSON.
The `native-angular1.log` run was deliberately stopped after its negative observation.
The final proof cases completed successfully. Prototype build failures and the tag/newer-asset
startup mismatch are also retained in the artifact root and are excluded from the measurements.
