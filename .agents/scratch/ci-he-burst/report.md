# CI HE flight admission

Subject: the video game's round flight, fuze and building destruction; game state only.

MEASURED from CI run 41793389428's saved log and the source at
"buildings: hull elements over the anchor revision, format 7": the failing assertion was
`assert_eq!(app.world().resource::<HeStress>().fired, 3)`, with actual zero. The following
burst assertion was never reached. Neither the gameplay-admission nor first-fire log appeared.
The brief's claim that firing succeeded was incorrect.

DERIVED cause: the test spent its update budget while the async spec load was pending.
`open_gameplay_gate` keeps the app in `Loading` until that load finishes; `GameplaySet` then
correctly prevents both the recipe and the round march from running. Update count bounds
simulated flight, not wall-clock asset IO. No bisection of the physics patch was needed once
the actual failing assertion was identified.

MEASURED Mac probes before the fix: isolated flight passed with compute pools of one, two,
four workers and the host-sized pool. The CI test profile also passed with one compute worker.
Temporary pool initialization and its environment variable were removed.

The regression wraps Bevy's real file reader only for the selected tank spec. Fixture: it
keeps that read pending until update 513, DERIVED as the original flight budget plus one.
The pending future yields its worker; other assets and tests remain runnable. The shipped
spec loader, admission gate, server simulation, real flight and destruction consumers stay
mounted.

```sh
OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  net::he_stress::tests::asset_io_may_outlast_the_flight_update_budget -- --exact --nocapture
```

MEASURED before the fix: the regression failed with fired zero, expected three, matching CI.
MEASURED after the fix: both normal and delayed admission produced three fired rounds, three
bursts and a nonempty fall ledger, without a tank or a Battle leaving `WaitingForPlayer`.

Decision: freeze simulation time while waiting for the shipped `Playing` gate, yield to IO,
and bound startup by a separate wall-clock watchdog with state and load-state diagnostics.
Fixture: watchdog 60 seconds, IO yield one millisecond. Assert no firing during frozen
admission, then restore the harness tick and retain the original 512-update flight budget.
Every production path, fuze law, asset and Avian patch is unchanged.

MEASURED gates: three HE tests passed in both edit and CI test profiles; the latter ran with
two test threads. All 48 whole-tree guard/bake tests, formatting, diff whitespace checks and
locked workspace/all-target CI-feature clippy passed. The changed tests were checked against
all review lenses; the backlog has no row naming this file.

Hosted exposure: this defect is confined to a test's startup budget. The CI failure supplies
no evidence of a Linux flight or fuze defect. No hosted deployment or Linux gameplay run was
performed, and no upstream draft needed changing.
