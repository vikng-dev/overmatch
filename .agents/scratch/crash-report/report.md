# Hosted server panic evidence

The shipped hosted entry installs the fatal hook before app construction; local servers keep their hook.
The first panic preserves default output and sends its message, file/line/column and forced backtrace.
Resolution: one whole-hook deadline is 15 seconds, observed by a prestarted watchdog every 10 ms.
It arms before output/capture/locks/allocation; `_exit(101)` bypasses runtime cleanup and blocked stdio.
Normal shutdown waits for an armed panic; secondary panics send nothing and cannot shorten its budget.
Completed failed sends attempt one failure line; the emergency deadline itself performs no I/O.

The authenticated `/end` accepts optional crash text; migration `0004_server_crashes.sql` archives it
independently of deployment discovery/log capture, using the log sanitiser, byte cap and retention.
Session reason: `the server crashed`, outcome `failed`; older end requests retain their behaviour.
Concurrent Registry stop callers join one deployment attempt through capture and confirmed stop.
Stopping claims remain owned, so the orphan sweep cannot bypass their active capture.
Apply the migration and deploy the Worker before the next game build; this lane deploys nothing.

Read both texts with the joined D1 command in `backend/README.md`; query `server_crashes` by `server_id`
when no log row exists. Lost: OS kill/OOM/native faults, network/D1 failure, redacted/cut text, late logs,
and a report/output prevented by blocked default output or the whole-hook deadline.
DERIVED: release stripping removes most Rust frame names; panic source location remains. Profile unchanged;
README records the server-only symbol-retention change and its unmeasured size cost.

MEASURED: original full-pipe, clean-finish and concurrent-stop/sweep regressions failed, then passed.
MEASURED: deleting only the first fatal exit failed the single-panic test after its 2 s allowance.
MEASURED: 69 Rust checks and 113 backend tests passed; per-file/workspace fmt, CI clippy and typecheck passed.
