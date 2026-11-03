# Hosted server panic evidence

The hosted entry installs a fatal panic hook before app construction; local servers keep their hook.
It preserves default output and reports the first panic's message, file/line/column and forced backtrace.
Resolution: the report helper gets a 15-second wait, including symbolication and HTTP.
Concurrent panics send once; either deadline can log the single failure line before exit.
The hook terminates even an isolated-thread panic when the Worker fails; no gameplay protocol change.

The existing authenticated `/end` accepts optional crash text. Migration `0004_server_crashes.sql`
stores it independently of log capture/deployment discovery, using the log sanitiser and byte cap.
The session reason is `the server crashed`, outcome `failed`; older end requests keep their behaviour.
Apply the migration and deploy the Worker before the next game build; this lane deploys nothing.

Read crash text and the container-log tail with the joined D1 command in `backend/README.md`.
If log capture has no row yet, query `server_crashes` by `server_id`; its retention follows log retention.
Lost: non-panic exits (OS kill/OOM/native faults), network/D1 failures, redacted/cut text, and late log lines.
A report deadline can expire before provider log capture finishes; crash text is saved before that fetch.
DERIVED: the shipped stripped release lacks most Rust frame names; panic source location remains.
The profile is unchanged. README records the server-only symbol-retention change and its unmeasured size cost.

MEASURED: 66 targeted/guard Rust tests passed; the two ignored child fixtures run through parent tests.
MEASURED: removing the shipped hook installation makes the entry-point test fail: no Worker request.
Gates passed: per-file rustfmt, workspace fmt check, CI clippy, backend typecheck and 111 backend tests.
