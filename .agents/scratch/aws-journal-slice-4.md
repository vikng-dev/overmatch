# Hosting slice 4: exit journal

Subject: hosting for Overmatch, a multiplayer tank video game. Lane `lane/host-journal`, bay-1.
No AWS/Cloudflare call, deployment, key operation, external comment or push ran in this lane.
The live proof below is instructions, not evidence. Issue #400's delayed empty exit is still
unexplained; this change supplies the missing archive for a subsequent run.

## Built contract

`Provider.logs` selects one capability: Edgegap pulls before stop; EC2 accepts a machine push.
No capture/admission branch names a provider. The embed evaluates the machine-owned grace
sum, so the Worker does not carry a second grace formula. One privileged
`ExecStopPost=+/usr/local/bin/om-harness hosted-stop` captures then requests
`systemctl --no-block poweroff`. An earlier asynchronous power-off hook would race capture,
so this unit has exactly one post-stop command. `After=network-online.target` gives shutdown
ordering against that target; actual network/DNS availability remains a live claim.

The machine reports `journalAvailable` on boot failures by testing whether it created the unit
file. False means immediate Worker termination and ticket replacement: there is no journal to
wait for. True gives the same grace as an ordinary server end. The server's normal/panic report
omits that field; its running process is itself proof that the unit was created. Boot callbacks
without a boolean fact get typed 400 before state changes; the parsed RPC type distinguishes
boot from process reports and names the meaning of process absence. Timing never infers unit creation.
After unit creation the boot trap synchronously stops the service before its fallback power-off.

Resolution bounds have one owner, the `JOURNAL_*` assignments in `machine.sh`, exported by
`embed-host.mjs` into `host.journal`:

| Bound | Job and derivation |
|---|---|
| 1 MiB | Resolution: bound pipe/redaction memory, upload admission and D1 archive storage. |
| 1,048,577 records | DERIVED: byte cap + one witness; even a record consisting of a newline costs one byte. `journalctl -b -n` bounds records before the byte-tail pipe, without excluding any record needed by the newest byte tail. Individual records can still be large; the outer kill bounds that case. |
| 12 s | Resolution: GNU `timeout --signal=KILL` kills the entire capture/redaction/upload process group. |
| 3 s / 8 s | Resolution: connection / total curl deadlines nested inside the whole-capture kill. One attempt, no retry. |
| 10 s | Resolution: margin for receipt of the end reply, process exit and shutdown/alarm scheduling. |
| 22 s | DERIVED Resolution: Worker grace = capture + reply/exit scheduling margin = 12 + 10. Upload is already inside capture. |
| 5 s | Resolution: local service process teardown and dispatch of the privileged post-stop hook. |
| 17 s | DERIVED Resolution: unit `TimeoutStopSec` = capture + local teardown/dispatch margin = 12 + 5. |
| 120 s | Resolution: admit a late shutdown push through scheduling/network jitter, without accepting old credentials indefinitely. |

A failed or stuck capture prints a fixed local diagnostic then still requests power-off. The
service stop timeout bounds service/process teardown as well. Worker `stop_at` is durable,
survives eviction, never moves on repeated `/end` (ordinary or late panic), rejects heartbeats
with 410 while stopping, and terminates through its alarm even without a push. The sweep
continues to regard the stopping deployment as owned. Other stop causes retain their immediate
provider termination path. Hard machine loss or unavailable networking can lose a tail.

## Admission, storage and failure outcomes

`POST /v1/servers/{id}/journal` uses the existing report bearer from the root-only header file.
No second credential, AWS permission, credential-bearing URL or public crash endpoint exists.
The Registry keeps only its SHA-256. It authenticates before HTTP reads bytes and again after
submission crosses RPC. Unknown row/deployment or a pull-only provider gets 410; wrong/missing
credential gets 401. Each valid push includes `X-Journal-Truncated: 0` or `1` and UTF-8 text.

Both `/journal` and `/end` enforce the machine-owned byte cap on the stream, before decoding or
concatenating an excessive chunk. For `/end` that cap includes the JSON envelope and escapes,
not just decoded crash text. Content-Length can refuse early, never bypass the counter. Invalid
length/oversize gets typed 413; invalid UTF-8, metadata, JSON or field shapes gets typed 400;
native body read failure gets typed 400. Cancellation does not await an untrusted producer.
The platform may already have allocated an incoming chunk; retained accumulated bytes are bounded.
Edgegap's pull already uses `providerText` to cap its raw response at 16 MiB before JSON parsing
and archive sanitisation (`SERVER_LOG_BODY_MAX_BYTES` owns that limit).

An authenticated own report credential is replaced in every occurrence: exact text, lower/upper
hex, either `0x` prefix case, base64 of the ASCII credential (both hex cases), and base64 of its
32 decoded bytes. The Worker warns with only the server row id, never the value, and saves the
archive. Its journal edge also removes these representations of the known netcode key. `/end`
removes own-report encodings before recording crash evidence. A machine never retries its push,
so refusal on credential presence would throw away diagnostics without extending protection:
the credential is unusable after its window closes anyway.

The first real archive wins using D1's unique server id; concurrent/later archives get 409.
An admitted push whose D1 write lands after the alarm replaces its NULL-log marker; tests exercise
both D1 write orders and refuse replacement of real archives. Replacement
and sanitisation happen before D1. A server process holding the report secret can forge a first
archive and end its own match; this is the accepted threat model, not evidence authenticity.
No claim of a trustworthy journal against a compromised server process is made.

The first stopping transition fixes `journal_deadline`; retries cannot extend it. Without a stop
transition, admission uses startup/last-heartbeat expiry plus the window. When the window closes,
the alarm inserts a `server_logs` row with `log NULL` and
`error = 'journal push window closed without an archive'`, unless a push already exists. Successful
archives are never overwritten. The check survives Registry eviction and D1 downtime: a failed
write keeps a durable deadline and retries on the production poll interval, independently of
machine termination/Fleet release. Unknown deployment fate stays pending until reconciliation
provides an id or proves refusal. A zero deadline means permanently closed and checked; retention
cannot delete a row whose archive check is pending. Legacy NULL deadlines retain the pre-stop
admission derivation. No additional database schema migration is required.

Expected D1 failures at both journal storage and crash recording cross Durable Object RPC as
`EvidenceResult` data, through the same `evidenceResult`/`evidenceCall` boundary, following
`FleetResult`/`fleetCall`. Diagnostics contain only local classifications and server ids; HTTP
returns typed 503. Failure causes carry the local operation name, distinguishing storage, Fleet
and history failures even when the wrapper covers the whole end operation. Crash-write failure still completes stop/ticket cleanup first. Bugs such as
missing tables still reject across RPC. Tests inject D1 network failures inside real DO methods,
then exercise the real HTTP caller; they also prove SQL bugs are not converted into downtime.

## Redaction and linear-time audit

Raw journal bytes stay in the bounded pipe. `tail` keeps cap + one truncation witness; a cut
first line is discarded because its field label may have been lost. Only redacted bytes and
metadata reach root-only `/run` tmpfs files. Redaction can expand short addresses, so both sides
cap the resulting UTF-8 tail again without splitting a code point. Hosted script/header/env
files remain root-only; systemd reads the environment before dropping to the service user.

Redact values, preserving labels and surrounding prose. `OVERMATCH_` assignments accept `=` or
`:`, masking the rest of the line; this also masks a configuration error's description after the
colon while keeping its variable name. Credential labels match after underscores (including
`report_secret`, `connect_token`, `tokens`), accept Basic/Bearer authorization and consume escaped
quotes without exposing the remaining value. Netcode's whitelist diagnostic keeps its message
and address-field labels while masking addresses. Multiline values, multiline Debug arrays,
hex-formatted arrays, comma-separated unquoted values and compound `tcp:peer:` addresses are not
claimed exhaustively masked; no emitter exists in `src/net` or vendored netcode. Live report/key
values are caught by their shapes. Both implementations state the multiline limit beside the
patterns and run the same reviewed raw/expected fixtures.

Source inspection: `src/net/hosting.rs` keeps key/report configuration behind
`Secret<T>::Debug` (`<redacted>`), and `ConfigError::Display` prints name and expected shape only.
Startup in hosting/server/server_host prints ids, origin, capacity, timing, bind and host facts,
never credentials. Vendored `lightyear_netcode-0.31.0-admission/src/server_plugin.rs` exposes the
key as `[u8; 32]` through `NetcodeConfig::Debug`; `auth.rs` also has a byte-array Debug field.
Therefore both sanitisers mask a bare 32-entry decimal byte array in addition to labelled key
values. No server log/config path base64-encodes that key: no speculative generic base64 pattern
is added, as stated beside the byte-array pattern in both implementations. Known own-credential
base64 encodings are removed explicitly at the authenticated Worker edge and for the machine's
report secret. Arbitrary unlabeled encodings or arbitrary panic text are not claimed exhaustively
parseable. No Rust source change was needed.

Audit of **every pattern** in the Worker and embedded Python sanitiser (n = bounded input bytes):

| Pattern family | Why it is linear |
|---|---|
| Exact provider/own credential replacement | A fixed number of literal searches; generated credentials have fixed length. The hex-shape check is fully anchored and fixed-width. |
| `OVERMATCH_...[:=]` | Fixed prefix at a word boundary; the name/spacing run cannot contain another eligible start. Its value is a newline-delimited single pass. |
| Credential labels/values | Finite fixed-length labels. Space runs cannot contain another label start. Value alternatives have distinct delimiters; bracket/object interiors exclude both opening and closing delimiters, preventing repeated suffix scans on unfinished nested dumps. Quoted values consume disjoint escaped-character/non-quote spans through an unescaped quote or fail once on an unfinished run. Unquoted values consume through whitespace/comma/semicolon. A fixed number of scans per span. |
| HTTP(S) URL | Fixed `http` start and a maximal non-space value; success consumes the complete run. No failing unbounded suffix after it. |
| Hex, including `0x` and adjacent non-hex letters | Negative hex lookbehind permits one attempt per maximal hex run, including `k` + hex. Optional `0x` is bounded, the final negative hex lookahead accepts the run end. It cannot retry at every hex digit. |
| Ticket/id | `(?<![A-Za-z0-9_-])` permits one start per maximal id run. The run excludes the literal dot; the hex suffix is fixed-width. In particular `a-a-a-...` no longer supplies a start at every word boundary. The same corrected pattern is used on both sides. |
| Decimal byte array | Literal `[` start, a fixed 32 entries and bounded digit widths. Commas partition entries; whitespace cannot contain another array start. Each span is scanned at most a fixed number of times, even on incomplete input. |
| IPv6 token scan | `[a-z0-9_:.%]+` always consumes a maximal token, with no failing suffix. A constant number of token-local passes identifies an optional non-hex label (`peer:`), validates the address alphabet, and counts colons. The anchored address/zone classes are separated by `%`, so cannot retry starts within the token. Rust paths and timestamps stay intact. |
| IPv4 | Four fixed-width digit groups and literal dots: constant work per start, including near misses. |

UTF-8 encode/tail/decode and first-line discard are linear too. Edgegap's larger bounded pull
uses the same audited Worker code. The byte-cap adversarial gate on both sides uses
`a-` repeated to exactly 1 MiB. Its **Resolution** budget is DERIVED as
`cap / (256 KiB/s) = 4 s`: deliberately slow throughput to tolerate CI/startup scheduling while
rejecting the former multi-minute quadratic scan. Worker clocks advance at I/O, so its test yields
before and after CPU work; the Python test times the actual installed `push-journal` command.

## Offline evidence

`machine.sh` owns its boot/command documentation and invariants, including the literal-path
delete reason. `embed-host.mjs` uses one Node comment-stripper; the harness invokes that same
owner once in preflight, before account reads, and carries the parsed script in its launch plan.
Missing Node or unsupported syntax is a visible `Refused` outcome. Shebang and settings marker survive. Comments inside quoted
source and static heredocs survive; unsupported quoting/substitutions refuse embedding. The
harness test compares its rendered script bytes with the generated Worker embed, and the Node
lexer tests cover quoted/heredoc comment lines and typed build refusals. The worst-realistic
launch test still requires at least 1 KiB headroom below EC2's 16 KiB cap.

MEASURED: backend typecheck passed; full backend tests passed (263 production-path, 126 EC2,
plus 2 Node comment-stripper tests),
`python3 scripts/aws/test_host_setup.py` passed (26 tests), and the harness passed (40 tests).
Shellcheck, `node backend/scripts/embed-host.mjs --check`, `cargo fmt --all --check` and
`git diff --check` passed. CI Clippy passed through the machine build queue after merging local `main`. Expected SQL/schema
and adapter invariant fixtures reject loudly; these are asserted failures, not unhandled test errors.

MEASURED at the 1,048,576-byte cap: the production TypeScript sanitiser bundled and run in
Node took 27.3 ms on the ticket adversary. The installed machine command, including fixture
process/I/O work, took 251.1 ms. These are local observations, not hosted CPU measurements;
both regression gates pass against the DERIVED 4,000 ms Resolution bound above. MEASURED real
`hostedScript` output for the stress fixture: 13,285 UTF-8 bytes, leaving 3,099 bytes against
EC2's 16,384-byte limit; documented `machine.sh` is 16,271 bytes. Launch bytes and credentials
are never printed by these measurements.

The shared `failD1Once(version, SQL fragment)` fixture injects failures inside the actual Registry
and restores its spy before throwing. Journal, end and recovery tests retain their HTTP/RPC and
cleanup assertions; SQL bugs still reject.

The hook tests run the actual installed
entrypoints and real GNU `timeout`, with journal/network/system commands replaced by local
fixtures. Both `timeout` and `gtimeout` are searched on PATH and checked for GNU coreutils.
Neither available is a hard failure naming `brew install coreutils` / `apt install coreutils`;
there is no hosting convention allowing a silent skip (unrelated Blender fixtures have their own
specific availability skips). The Python timeout replacement is deleted. Hung journal/upload
fixtures create descendants in the real capture process group; tests prove bounded completion,
no live descendants and a subsequent power-off request. All curl/service/deadline assertions read
the production owners and check nesting. The dead start-failure fixture assignment is removed.
These tests exercise the shell composition, not systemd's lifecycle manager.

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

7. Prove every remaining lifecycle case on separately launched proving matches, restoring the
   ordinary script between variants. Keep the single privileged hook intact; add only temporary
   proof commands/diagnostics, regenerate the embed, and remove them after the evidence is saved:

   - **Failed service start:** temporarily point `ExecStart` at a nonexistent absolute executable.
     Prove root's post-stop hook attempts upload, the boot callback reports unit-created true,
     and the fallback power-off follows the synchronous stop. Separately fail before unit creation
     (for example a deliberately wrong fixture checksum); prove no grace/Fleet delay and a visible
     missing archive outcome when its window closes.
   - **API shutdown:** separately authorize and invoke AWS API stop/termination for one ready match.
     Do not infer hook execution from Worker stop success: the sanitized archive must contain a
     temporary root-hook identity marker and the final server lines. Expect an API-initiated reason
     in this case, not the clean-end self-shutdown reason.
   - **Lifetime guard:** make CAP_MIN short only in a proof build, let shutdown trigger without
     `/end`, and prove the privileged hook archives before power-off. Check its archive timestamp
     and unit journal, not just instance disappearance.
   - **Capture deadline:** replace only proof `push-journal` work with a command whose descendants
     block. Temporarily record GNU timeout's exit status as a fixed numeric diagnostic in the
     outer hook (preserving the unconditional following power-off). Prove status 137, no live
     descendants, and power-off despite that status. A missing archive must acquire the Worker's
     NULL/error row after the admission window closes.
   - **Network/DNS during shutdown:** for the API/lifetime variants, require an actual successful
     HTTPS push to the Worker during shutdown; this proves both DNS and network stayed usable
     for the hook. A local curl-attempt line alone does not prove either.
   - **Journal flush:** emit a unique final nonsecret process line immediately before exit in a
     proof build; require it in each available archive. The unit manager's own final stopped line
     can follow capture. Record any missing final process line as an open flush/order failure.
   - **Root privilege:** for clean exit, SIGKILL, failed start, API shutdown and lifetime guard,
     capture a temporary numeric effective uid diagnostic from inside the hook; require uid 0
     together with successful archive reading. The unit-file `+` string alone is not evidence.
   - **Shutdown/billing:** for clean end, timestamp hook power-off, archive receipt, instance
     `shutting-down` and fallback alarm attempts. Require `Client.InstanceInitiatedShutdown` before
     the derived grace expires; API termination winning is inconclusive for this property. Verify
     the provider's billing boundary at `shutting-down` from billing/usage evidence, separately
     from instance disappearance; offline elapsed-time tests do not prove billing.
   - **Real user data:** measure only UTF-8 byte length of the actual rendered launch bytes before
     submission (never print the script/settings). Require <=16 KiB and record the count/headroom;
     fixture headroom is not proof of an operator's actual settings.

   Finish with ordinary proving bytes redeployed, no temporary uid/timeout markers, no timer,
   crash endpoint or retained experiment flag, and the zero-runtime-resource checks in step 5.
