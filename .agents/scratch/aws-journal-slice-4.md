# Hosting slice 4: exit journal

Subject: hosting for Overmatch, a multiplayer tank video game. Lane `lane/host-journal`, bay-1.
No AWS/Cloudflare call, deployment, key operation, external comment or push ran in this lane.
The live proof below is instructions, not evidence. Issue #400's delayed empty exit is still
unexplained; this change supplies the missing archive for a subsequent run.

## Built contract

`Provider.logs` selects exactly one capture capability: Edgegap pulls before stopping; EC2 accepts
a machine push. There is no provider-name branch in capture or admission. EC2's `/end` replies
without terminating immediately: its durable stop alarm waits 30 s (Resolution: let the process
receive the reply, exit, and finish the bounded capture). The sweep still considers the stopping
row owned. That grace survives eviction, does not extend on duplicate end reports, and ends even
if no push arrives. Other stop causes retain their provider termination path.

One privileged `ExecStopPost=+/usr/local/bin/om-harness hosted-stop` captures, then requests
`systemctl --no-block poweroff`. There is no earlier asynchronous power-off command to race it.
It runs for clean exit, process failure, failed service start and the lifetime shutdown. A boot
failure after unit creation synchronously requests its stop before the boot trap's fallback
power-off; a boot failure before the unit exists has no unit journal. `After=network-online.target`
keeps shutdown ordering against the network target. The machine still has no SSH or instance role.

Resolution bounds: whole capture 12 s, enforced by coreutils `timeout --signal=KILL` against the
capture process group; curl connects within 3 s and finishes within 8 s; `TimeoutStopSec=30s`
bounds process/service stop. The shorter capture budget leaves teardown time inside the service
budget. One attempt covers journal read, redaction and upload; any failure loses that archive,
prints a fixed local diagnostic and continues to power-off. No retry, queued delivery or new
operator switch exists. An OS power failure, broken network or unavailable Worker can lose a tail.

This macOS host has no `systemd.service` man page (`man -w systemd.service` reports none).
**Claims to prove live:** systemd runs post-stop commands for failed starts as well as normal
termination; multiple `ExecStopPost=` commands execute in declaration order (a failed unprefixed
command skips later commands); `TimeoutStopSec` applies separately to stop commands and bounds
waiting for the process, and also governs post-stop execution. This implementation uses one
post-stop command, so it does not rely on a second command surviving a first command's failure.
The `+` command prefix restores the privileged execution needed to read the journal and power off.
Network shutdown ordering and Ubuntu's Python/coreutils availability also remain live claims.
Reference for the operator's version check: `man systemd.service`, `man systemd.exec`,
`man systemd.unit`, `man timeout` on the proving machine's image.

## Admission and security

The route is `POST /v1/servers/{id}/journal`, with the existing
`Authorization: Bearer <OVERMATCH_REPORT_SECRET>` header from `/etc/overmatch-report-header`.
The server's heartbeat/end requests and the boot failure callback already use this credential.
No second credential, key in a URL, AWS permission or public crash endpoint is added. Registry
stores only its SHA-256; the authenticated supplied credential lets it check the raw journal for
that exact value without persisting it.

Registry admission runs before HTTP reads the body and again on submission. Unknown row: 410;
wrong/missing credential: 401; a provider without push, or an unrecorded deployment: 410. The
window closes 120 s after the **first** stopping transition (Resolution: shutdown scheduling and
network jitter); stop retries cannot move it. Without a stop transition, admission closes 120 s
after startup expiry or the latest heartbeat expiry, allowing an unreported spontaneous crash.
The nullable `journal_deadline` column is added to Registry's own SQLite schema on construction;
pre-existing rows use their last plausible heartbeat/start expiry. No D1 migration is required.

A declared Content-Length over 1 MiB refuses before reading; a missing or dishonest length still
hits the streamed byte counter before decoding/concatenating an over-limit chunk. Invalid length
and oversized body: 413; invalid UTF-8/metadata or raw own credential: 400. Each refusal is typed
and logs a fixed reason/status, never payload, authorization or URL. Native body read failure is
also a typed 400. Stream cancellation does not block the refusal. The chunk delivered by the
platform has already been allocated; the Worker bounds its own accumulated data, not that chunk.
A valid request requires `X-Journal-Truncated: 0` or `1` and plain UTF-8 text.

The first valid push inserts `server_logs`; any second/concurrent push gets 409. Refusal rather
than success distinguishes a duplicate from a newly saved archive and prevents replacing the
first account of an exit. A rejected credential-bearing body consumes no archive slot. The row
uses the recorded `deployment_id` (EC2 instance id), preserving the existing primary/unique keys
and README join with `server_crashes`. Existing archive retention applies. Unexpected SQL/schema
failures propagate; only recognized infrastructure failures take the existing typed 503 path.

## Redaction contract and source inspection

Raw journal bytes remain in a pipe. `tail` retains at most the byte cap plus one truncation witness;
if it cuts a line, the redactor discards that incomplete first line so a lost label cannot expose a
partial credential. Only redacted bytes and the truncation header reach root-only files on `/run`
(tmpfs); curl reads those files and the existing root-only authentication header. The installed
hosted script is root-only. The sanitized UTF-8 tail is capped again after redaction, since masking
short addresses can expand the output. The Worker rejects a raw occurrence of the authenticated
server credential, then applies its archive sanitizer before any durable log write.

Both archive redactors remove these forms (case insensitive except the exact credential):

- The machine's exact report credential, even unlabelled; the Worker refuses that exact value.
- Whole lines containing `ticket`, `OVERMATCH_`, `secret`, `token`, `password`, `authorization`,
  `private[_ -]?key`, `netcode[_ -]?key`, `env_vars` or `environment`. This covers labelled byte-array
  key dumps, tickets, command-line/environment dumps and authorization headers.
- HTTP(S) URLs through the next whitespace, including the callback origin/path.
- Contiguous hex credentials of at least 64 digits, including the generated netcode key.
- Versioned id/ticket forms `[A-Za-z0-9_-]+\.[0-9a-f]{32}` (also masks matching server ids).
- IPv4 tokens, IPv6 tokens including mapped IPv4, compressed/loopback and zone-id forms.

Worker sanitization also removes its configured provider token. These patterns are a diagnostic
redaction contract, not a parser for every conceivable encoding of a secret. The exact own-secret
refusal protects against missing generic recognition; arbitrary base64/escaped unlabeled dumps
are not claimed covered. Both suites exercise every listed form and preserve paths/timestamps/stats.

Inspected `src/net/hosting.rs` and `src/net/server.rs`, plus `server_host::log_startup` and the
vendored netcode startup path. Startup logs the server id, Worker API origin, capacity,
heartbeat/empty-exit durations, listening bind/port, and host CPU/task-pool/cgroup facts. It prints
no netcode key, report credential, player ticket or environment dump. `Secret<T>::Debug` prints
`<redacted>`; configuration errors name the variable/shape without its value. The callback origin
is public configuration, not a credential; URLs and addresses are removed from the archive.
No secret-at-start finding required a Rust change. Arbitrary panic text and peer-address logs are
why redaction still belongs at the archive edges.

## Offline evidence

MEASURED local results: `npm ci --prefix backend --ignore-scripts --no-audit --no-fund` passed;
backend typecheck passed; full backend tests passed (247 production-path, 114 EC2). The host-setup
suite passed (26 tests); the harness suite passed (34 tests, including real capture deadlines).
Shellcheck on `machine.sh`, embedding `--check`, `cargo fmt --all --check` and `git diff --check`
passed. Expected invariant-failure fixtures log exceptions and assert their rejection; there were
no unhandled test-run errors. No Rust changed and no compilation or deployment ran. The Python machine fixture runs
the generated script's real command entrypoints with OS/network commands replaced on PATH. On
macOS a Python timeout shim enforces a real process-group kill deadline; production uses GNU
coreutils timeout. These tests do not claim to emulate systemd's lifecycle manager.

## Live proof for the orchestrator (not run)

Prerequisites: separately authorized proving deployment and existing hosting credentials/network;
production remains Edgegap. Slice 3's second attempt ended keyless, so do not mistake its current
credential refusal for a journal defect. Drain unrelated proving matches first. Use a leased bay,
no shell tracing, no environment/credential dump, and no secret files. All following cloud commands
are operator work outside this lane. Existing archive/crash migrations must already be applied.

1. From the repo root, run `node backend/scripts/embed-host.mjs --check`. From `backend/`, deploy
   the ordinary build with `npx wrangler deploy --env aws-proving`. Inspect `/admin` under Access
   for an empty Fleet and note its public Worker origin. Set `OVERMATCH_API` to that origin and
   `OM_PROOF_VERSION` to a published server version (for example the already proved alpha.20).
   No report secret is needed by the operator.

2. Request one empty match with the following local Python command. It holds the ticket/key only
   in memory and prints only state. Do not run clients; the shipped empty-exit policy ends it.

   ```sh
   python3 - <<'PY'
   import json, os, time, urllib.request
   api = os.environ['OVERMATCH_API'].rstrip('/')
   def post(path, body):
       request = urllib.request.Request(api + path, data=json.dumps(body).encode(),
           headers={'Content-Type': 'application/json'})
       with urllib.request.urlopen(request, timeout=15) as answer:
           return json.load(answer)
   ticket = post('/v1/join', {'version': os.environ['OM_PROOF_VERSION'],
       'locations': ['Singapore'], 'pings': {}})['ticket']
   for _ in range(300):  # Proof-only bound: wait at most 5 minutes for startup.
       state = post('/v1/join/status', {'ticket': ticket})['state']
       if state != 'starting':
           print(state)
           break
       time.sleep(1)
   else:
       raise SystemExit('startup did not finish within the proof window')
   PY
   ```

3. Wait for the row to end and the archive to appear. From `backend/`, identify this run's
   `deployment_id`/`server_id` via Access `/admin` and the proving sessions query below. No ticket,
   credential, key or raw environment is selected. `overmatch-sessions-aws-proving` is the proving D1
   binding's database name; keep `--env aws-proving` on every command.

   ```sh
   npx wrangler d1 execute overmatch-sessions-aws-proving --env aws-proving --remote --json --command "SELECT server_id, deployment_id, outcome, reason FROM sessions ORDER BY requested_at DESC LIMIT 2;"
   npx wrangler d1 execute overmatch-sessions-aws-proving --env aws-proving --remote --json --command "SELECT crash, log, error FROM server_logs LEFT JOIN server_crashes USING (server_id) WHERE deployment_id = '<instance-id>';" | jq -r '.[] | .results[] | (.crash // empty), (.log // .error // empty)'
   ```

   Expected clean row: one `server_logs` row for that instance id and server id, non-NULL `log`,
   NULL `error`, `truncated` either 0 or 1; session outcome `ended`, reason containing the server's
   empty exit. The log should include the last empty-exit/report lines; systemd's final successful-stop line
   can follow the snapshot and be absent.
   `server_crashes.crash` is NULL. Check the listed sensitive forms are absent. Compare timestamped
   client/empty/report lines to investigate #400; do not assume its former timer was correct.

4. Force a **process crash**, without SSH, SSM or additional IAM actions: in this proving bay only,
   make the following temporary boot-script edit. It schedules root's systemd kill against the
   server process 90 s after start (proof-only scheduling delay), leaving the stop hook intact.
   SIGKILL avoids a memory-bearing core dump. This proves abrupt process exit, not the Rust panic
   hook; an authenticated panic should additionally leave `server_crashes`, as offline tests show.

   ```sh
   python3 - <<'PY'
   from pathlib import Path
   path = Path('scripts/aws/machine.sh')
   source = path.read_text()
   anchor = 'ExecStart=$INSTALL/overmatch-server\n'
   assert source.count(anchor) == 1
   source = source.replace(anchor, anchor + 'ExecStartPost=+/usr/bin/systemd-run --unit=overmatch-proof-crash --on-active=90s --timer-property=AccuracySec=1s /usr/bin/systemctl kill --kill-whom=main --signal=KILL overmatch-server.service\n')
   path.write_text(source)
   PY
   node backend/scripts/embed-host.mjs
   ```

   From `backend/`, `npx wrangler deploy --env aws-proving`, then repeat step 2 once. Repeat step 3
   for the new instance id. Expected crash archive: exactly one `server_logs` row, non-NULL redacted
   `log`, NULL `error`, with systemd reporting `code=killed` and `status=9/KILL`. A SIGKILL cannot
   send `/end` or panic text: `server_crashes` can be absent; session reason may be lost heartbeat
   or provider termination. The machine's own stop hook must still archive then power off.

5. Read each instance promptly after termination with the existing operator identity (from repo
   root). This projection prints only state/reason, never the full response:

   ```sh
   aws --profile overmatch --region ap-southeast-1 ec2 describe-instances --instance-ids '<instance-id>' --query 'Reservations[].Instances[].{state:State.Name,guard:StateReason.Code}'
   aws --profile overmatch --region ap-southeast-1 ec2 describe-instances --filters Name=tag:Project,Values=overmatch Name=tag:Purpose,Values=host Name=tag:WorkerEnvironment,Values=aws-proving Name=instance-state-name,Values=pending,running,shutting-down,stopping,stopped --query 'length(Reservations[].Instances[])' --output text
   aws --profile overmatch --region ap-southeast-1 ec2 describe-volumes --filters Name=tag:Project,Values=overmatch Name=tag:Purpose,Values=host Name=tag:WorkerEnvironment,Values=aws-proving --query 'length(Volumes)' --output text
   aws --profile overmatch --region ap-southeast-1 ec2 describe-network-interfaces --filters Name=tag:Project,Values=overmatch Name=tag:Purpose,Values=host Name=tag:WorkerEnvironment,Values=aws-proving --query 'length(NetworkInterfaces)' --output text
   ```

   Expect `terminated` / `Client.InstanceInitiatedShutdown` for the hook, and zero for all three
   runtime counts; `/admin` Fleet must return empty. `Client.UserInitiatedShutdown` means the API
   won, not proof of the hook; an absent/other reason is inconclusive. Archive absence, surviving
   resources, a credential-bearing row, or only fallback API termination leaves that proof open.

6. Restore **only** the proof edits inside this bay:
   `git restore -- scripts/aws/machine.sh backend/src/host.generated.ts`, check embedding, and
   deploy the ordinary proving build again. Do not leave a timer, runtime switch or crash endpoint
   in the shipped code. Drain before ending the operator session. Standing key handling remains
   the orchestrator's existing authorization, not a permission added by this slice.
