# Hosted server panic evidence

The shipped hosted entry installs the fatal hook before app construction; local servers keep their hook.
The panic owner preserves default output and sends its message, file/line/column and forced backtrace.
Resolution: the whole-hook deadline is 15 seconds; a prestarted watchdog blocks until armed, without polling.
It arms before output/capture/locks/allocation; `_exit(101)` bypasses runtime cleanup and blocked stdio.
Panic and clean shutdown share one atomic end claim; an armed panic cannot be overtaken during reporter joining.
A panic after the clean claim sends no second end; it still fails the process, but crash text may only be local.
Completed failed sends attempt one failure line; secondary panics cannot shorten the deadline or send twice.

The authenticated `/end` accepts optional crash text; migration `0004_server_crashes.sql` archives it
independently of deployment discovery/log capture, using the log sanitiser, byte cap and retention.
Session reason: `the server crashed`, outcome `failed`; older end requests retain their behaviour.
Concurrent Registry stop callers join one capture/stop attempt; the orphan sweep respects its stopping claim.
Apply the migration and deploy the Worker before the next game build; this lane deploys nothing.

Read both texts with the joined D1 command in `backend/README.md`; query `server_crashes` by `server_id`
when no log row exists. Lost: OS kill/OOM/native faults, network/D1 failure, redacted/cut text, late logs,
and a report/output prevented by blocked default output or the whole-hook deadline. Raw output is not sanitised.
DERIVED: release stripping removes most Rust frame names; panic source location remains. Profile unchanged;
README records the server-only symbol-retention change and its unmeasured size cost.

MEASURED: removing only the clean end claim made the reporter-join race fail with an early clean `/end`.
MEASURED: deleting only the first fatal exit failed the single-panic test after its 2 s allowance.
MEASURED: 70 Rust checks and 113 backend tests passed; per-file/workspace fmt, CI clippy and typecheck passed.
