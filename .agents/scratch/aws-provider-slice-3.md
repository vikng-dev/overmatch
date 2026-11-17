# Hosting slice 3: EC2 provider error-rule closure

Subject: hosting for Overmatch, a multiplayer tank video game. Building lane `lane/aws-provider`,
bay-3. This revision made no deployment, key handoff, AWS request, quota request or external comment.
Only local checks and public primary-source reads ran. Production still selects Edgegap.

## Error-rule closure

Decision: use one rule across every Worker catch in the lane's diff. Parse outside failures once
at their native I/O, JSON, response-schema or persisted-cache edge; catch only those typed errors
for refusal, a safe cause log, durable retry or unknown-outcome reconciliation. An unexpected
exception propagates and fails the invocation. Native response-stream errors have no stable
platform subtype: their small, I/O-only broad catch states that reason beside it. The batch runner
collects all task failures and rethrows them; HTTP and scheduled entry points have no swallowing catch.

This **supersedes the earlier instruction and Security P3-9 disposition to restore main's broad
deploy catch**. It is a small production change for **Edgegap**, which remains the selected provider:
a malformed/null deploy or list body is now typed at the adapter edge. A deploy's unknown outcome
is reconciled by tag as before; malformed inventory cannot prove absence. An internal bug now fails
the request instead of leaving a ticket waiting behind an `unknown` result. Status, location,
beacon and log bodies are validated at that same edge; ordinary JSON is bounded before parsing.

Fleet returns typed failures only for documented platform reset/overload flags; missing tables,
constraints and other SQL invariants propagate. RPC deadlines/transport faults are classified
separately from remote exceptions. D1 has message-only infrastructure faults, so its edge accepts
only the documented exact `D1_ERROR` causes; schema errors stay bugs. Evidence and sweep-state
handling follow the same rule. The admin snapshot displays transport unavailability, while snapshot
invariants fail the request; the launch proof catches only ProviderError. Cached release/catalog
JSON from earlier builds is parsed and shape-checked once, producing a typed visible refusal if invalid.

The two brevity P3s are closed: `Fleet.slots` is inlined into the storage callback, and updater
failures share an environment-value subtest table. The retired `+` metadata-success test and its
fixture override are deleted. The hosted success fixture uses one supported release filename and
still verifies the fixed local archive before unpacking/removal. The literal absolute `rm` stays:
the delete rule takes precedence over the lens; its inline comment explains that a changed variable
must not redirect deletion. The orphan test admits one live host, then seeds orphan machine/slot
state and asserts cleanup/preservation by identity. It also passes with `maxServers` temporarily
set to one; the tracked configuration was restored immediately after that check.

### Complete catch audit

Line numbers name the final source. Every catch in every source file in the lane's diff was examined;
other edited source/scripts/tests contain no JavaScript catch. The additional evidence tests were
updated because they now distinguish documented D1 transport faults from schema bugs.

| File:line | What it catches | Typed or propagated |
|---|---|---|
| `backend/src/registry.ts:194` | catalog read for locations | ProviderError: visible location refusal; bugs propagate |
| `backend/src/registry.ts:220` | join catalog read | ProviderError: 503 refusal; bugs propagate |
| `backend/src/registry.ts:229` | join release verification/cache read | ProviderError: 400/404/503 refusal and safe cause log; bugs propagate |
| `backend/src/registry.ts:316` | webhook status read | ProviderError: log cause and leave alarm recovery; bugs propagate |
| `backend/src/registry.ts:397` | alarm task | ProviderError: log durable retry; bugs propagate into the batch rejection |
| `backend/src/registry.ts:446` | release verifier | Only releaseMissing becomes the negative cache; all other errors propagate |
| `backend/src/registry.ts:469` | catalog refresh | ProviderError: log cause and serve a parsed stale catalog, or propagate without cache; bugs propagate |
| `backend/src/registry.ts:578` | deploy | ProviderError: definite refusal or unknown outcome reconciled by tag; bugs propagate |
| `backend/src/registry.ts:603` | forgotten claim stop | ProviderError: log cause; next sweep can retry the still-listed host; bugs propagate |
| `backend/src/registry.ts:647` | claim lookup | ProviderError: log cause, retain durable reconciliation; bugs propagate |
| `backend/src/registry.ts:680` | status poll | ProviderError: log cause, retain durable poll; bugs propagate |
| `backend/src/registry.ts:831` | deployment stop | ProviderError: log cause, retain durable stopping row; bugs propagate |
| `backend/src/registry.ts:845` | Fleet release | ProviderError: log cause, retain stopping row and tickets; bugs propagate |
| `backend/src/fleet.ts:19` | provider inventory | ProviderError becomes logged FleetResult provider failure; bugs propagate |
| `backend/src/fleet.ts:45` | local SQL storage | Only documented retryable/overloaded flags become logged FleetResult storage failure; SQL/schema bugs propagate |
| `backend/src/sweep.ts:77` | inventory listing | ProviderError: log cause and record failure for a later sweep; bugs propagate |
| `backend/src/sweep.ts:101` | host status | ProviderError: log cause and record failure for a later sweep; bugs propagate |
| `backend/src/sweep.ts:131` | Registry ownership RPC | ProviderError: log cause and record failure for a later sweep; bugs propagate |
| `backend/src/sweep.ts:147` | host stop | ProviderError: log cause and record failure for a later sweep; bugs propagate |
| `backend/src/sweep.ts:166` | individual reservation cleanup | ProviderError: log cause and record failure for a later sweep; bugs propagate |
| `backend/src/sweep.ts:168` | reservation inventory | ProviderError: log cause and record failure for a later sweep; bugs propagate |
| `backend/src/sweep.ts:193` | sweep-state write | Typed D1/transport/deadline failure logs cause; bugs propagate |
| `backend/src/sweep.ts:203` | sweep-state read | Typed D1/transport/deadline failure logs cause and shows no snapshot; bugs propagate |
| `backend/src/ec2.ts:65` | operator network JSON | SyntaxError/schema refusal becomes ProviderError config; bugs propagate |
| `backend/src/ec2.ts:131` | signed HTTP/XML query | DeadlineError becomes typed unknown outcome; every other exception propagates |
| `backend/src/ec2.ts:139` | bounded deploy | DeadlineError becomes typed unknown outcome; every other exception propagates |
| `backend/src/ec2.ts:161` | launch attempt | ProviderError controls bounded retransmission/capacity handling; bugs propagate |
| `backend/src/ec2.ts:181` | dry-run query | Only typed DryRunAccepted confirms proof; every other exception propagates |
| `backend/src/ec2.ts:242` | instance status | Typed MissingInstance is unknown until absence deadline, then gone; others propagate |
| `backend/src/ec2.ts:259` | instance stop | Typed MissingInstance is unknown until absence deadline, then confirmed absent; others propagate |
| `backend/src/edgegap.ts:63` | bounded exchange and parsed-response handler | Only DeadlineError becomes typed unknown outcome; all other errors propagate |
| `backend/src/provider.ts:110` | native fetch only, after argument validation | Network TypeError/AbortError/TimeoutError becomes ProviderError unknown; other errors propagate |
| `backend/src/provider.ts:119` | outside JSON syntax | SyntaxError becomes ProviderError unknown; other errors propagate |
| `backend/src/provider.ts:141` | bounded RPC | DeadlineError/documented retryable or overloaded flags become ProviderError transport; bugs propagate |
| `backend/src/provider.ts:165` | D1 operation | Exact documented D1_ERROR infrastructure messages become ProviderError storage; all other errors propagate |
| `backend/src/provider.ts:208` | fatal UTF-8 decode only | Native decoder TypeError becomes ProviderError; other errors propagate |
| `backend/src/provider.ts:230` | native network stream read/cancel only | Broad: platform stream errors have no stable subtype; become ProviderError, with the reason stated inline |
| `backend/src/release.ts:30` | release HTTP/checksum verification | Only DeadlineError becomes releaseUnavailable; every other exception propagates |
| `backend/src/index.ts:29` | client JSON body | Only SyntaxError becomes the visible malformed-body refusal; bugs propagate |
| `backend/src/index.ts:151` | launch proof | ProviderError: log safe cause and refuse proof; bugs propagate |
| `backend/src/admin.ts:28` | Access JWT/JWKS verification | JOSEError/ProviderError: visible Access refusal; bugs propagate |
| `backend/src/admin.ts:87` | Registry snapshot | ProviderError transport/deadline: log cause and show snapshot unavailable; bugs propagate |
| `backend/src/evidence.ts:59` | capture/marker write | ProviderError: log cause, persist failure and continue stop; bugs propagate |
| `backend/src/evidence.ts:69` | failure-record write | ProviderError: log cause and continue stop; bugs propagate |
| `backend/src/evidence.ts:83` | retention write | ProviderError: log cause; later sweep retries retention; bugs propagate |
| `backend/src/ids.ts:27` | outside id decoding | Native InvalidCharacterError/UTF-8 TypeError becomes malformed id, visibly refused by routing; bugs propagate |
| `backend/src/deadline.ts:33` | independent cleanup task | Every rejection collected, then propagated in AggregateError after the batch completes |
| `backend/src/index.ts:168` (removed HTTP catch) | route/config exceptions | Propagated directly to the platform; no synthetic successful invocation with a 500 response |
| `backend/src/index.ts:174` (removed scheduled catch) | sweep exceptions | Propagated directly; invariant failures fail the scheduled invocation |

## Prior review dispositions

The two supplied reviews were read in full and checked against source. Finding numbers here refer
to `lenses-provider-2.md` and `review-provider-2.md`, respectively.

| Finding | Decision and evidence |
|---|---|
| Lens error P1, Fleet release | Fixed: Fleet returns typed storage/provider failures as RPC data; its adapter classifies retryable/overloaded platform faults and deadlines. `markEnded` persists its next attempt before release, logs the safe cause, and retains `stopping` and tickets on failure. Eviction/recovery is tested. `settleAll` completes the batch and rejects with every unhandled failure; invariants cannot disappear. |
| Lens error P2, launch time; security P3-8 | The security review described the reachable code correctly: missing/unreadable time failed the entire EC2 listing. The lens described a latent undefined-time guard additionally swallowed by the task runner. Fixed: capped inventory requires time parsed at the EC2 edge, either a timestamp or named `unreadable`. Bad-time hosts count toward admission, log by host, and still receive ownership/orphan checks; other hosts proceed. Both bad-time forms and live ownership are tested. |
| Lens error P2, server id | Fixed: reserve rejects malformed ids before EC2 reads or SQL persistence. Sweep catches typed outside failures only; corrupt stored ids reject loudly, tested separately from a storage outage. |
| Lens error P2; security P3-3 | Fixed: every masked updater must report `inactive`, `failed` or `LoadState=not-found`. Other/empty states, including `activating`, and failed queries report `boot-updates`, refuse play and power off. Tests run the shipped script. |
| Lens brevity, archive | Partly fixed: checksum uses `$archive`; unnecessary `cd` removed. Variable-path deletion rejected because this task requires a literal absolute delete path; literal `rm` retained. |
| Lens brevity, fake zone | Fixed: removed stored `Machine.zone`; local zone still scopes ClientToken idempotency. |
| Lens testing; security P3-10 | Fixed: fake never synthesizes an environment tag; owned fixtures specify `env.WORKER_ENV`. Untagged hosts remain invisible, explicitly tested and the reason for step 0. EC2 request locations read `hosting.location.city`. The orphan machine/slot assertion is now independent of the configured cap, as described above. |
| Security P3-1 | Fixed step 0: Project-only count across all purposes/environments, zero runtime resources, then standing `verify`. Standing group cannot also be zero without contradicting verification. |
| Security P3-2 | Fixed: Registry alarms expire all live rows at the same provider-owned lifetime plus margin, independent of heartbeats. Expiry detaches/fails tickets before outside work, then confirms stop/release through durable retries. Heartbeats enforce expiry; placement excludes expired rows. |
| Security P3-4, P3-5 | Accepted live limits: updater stop latency and the no-home server user are unmeasured until step 4; it records boot-attempt count and duration. No invented live result. |
| Security P3-6 | Deferred by instruction: rate limits for `/v1/join` and `/v1/locations` remain an open cutover item. Distinct release versions can still drive GitHub reads and availability throttling. |
| Security P3-7 | Fixed: margin labelled Resolution for clock skew and boot time before the machine timer. Sweep and Registry read one `inventory.maxAgeMs` owner. |
| Security P3-9 | Superseded by the owner's error rule above: malformed Edgegap responses are typed at the edge and reconciled; internal deploy exceptions propagate. Unsupported `/admin/aws-proof` still returns 404 before Access/body checks. |
| Security P3-11 | Fixed: added tested `revoke-key --env`: teardown removes standing parts and rotation creates a key, so neither does exactly what step 7 needs. Revocation retains user/policy/group for the next slice. |
| Security P3-12 | Fixed conservatively: stable-tag pattern excludes `+`, but the prerelease wildcard can admit it; upload-server uses raw tags. That proves the local name, not GitHub's asset name. EC2 refuses `+` with `releaseBuildMetadataUnsupported` before network/persistence. Edgegap's version syntax stays unchanged. |

The Edgegap `findByTag` response-shape finding is closed by edge parsing; malformed inventory remains unknown rather than proving absence.
No backlog rows name the edited files. No agents, live AWS calls, account-changing commands,
external comments or deployments ran. Independent pre-merge lenses remain the orchestrator's work.

## Provider interface and ownership

`Provider` owns deploy, find-by-tag, paginated inventory, status, confirmed stop, locations and
beacons, with optional log pull. Its policy is expressed as:

- `starts`: attempt budget and whether boot failures retry in the same location after confirmed stop;
- `inventory`: whether listings establish ownership even without a valid claim, and an optional
  maximum age shared by sweep and Registry; capped listings require parsed `launchedAt` or named `unreadable`;
- `requiresHttps`, `usdPerVcpuMinute`;
- optional `verifyRelease`, `reservations`, `releaseClaim`, and exact-shape `dryRun` methods.

The factory is the single provider-name owner. Deleting Edgegap removes its implementation/import
and factory entry, and changes production's config value; shared lifecycle code requires no search
for Edgegap branches. The existing Edgegap webhook HTTP contract remains until cutover removes it.
`/admin/aws-proof` has a named next user: the cutover slice's native/concrete-image launch verification.
It remains Access-protected and creates neither a machine nor a Fleet slot.

`backend/aws-hosting.json` owns cap, start attempts, lifetime, sweep margin and regional display
metadata. The margin is a Resolution number: five minutes for clock skew and boot time between EC2 launch
and the machine's own power-off timer; this is not a measured cloud termination bound. The sweep's cap is independent of the
machine's scheduled shutdown. Failed AWS/sweep operations still defer cleanup; neither guard is a
spend ceiling. The root contract and launch constraints remain owned by setup/policy data.

## IAM and deployment identity

**Policy unchanged; `host-setup apply` does not need to be re-run for these fixes.** The actual
policy template has no `aws:TagKeys` allowlist and permits extra creation tags. Its RequestTag
conditions require Project/Purpose; CreateTags is restricted to RunInstances. Existing simulation
cases still test that contract, and `test_host_setup.py` passes. No AWS permission was added.

`WORKER_ENV` is an immutable operator identity, distinct for every Worker. Never reuse it for a
second live deployment. Drain before changing it: older machines without this tag, or with the
previous identity, are deliberately outside the new inventory. Before a first deployment of this
revision, check all host resources read-only and confirm that no earlier untagged hosts remain.
The standing setup user/key is still shared infrastructure: environment isolation does not authorize
issuing another key or running teardown while another environment is active. A stolen shared key
is not IAM-isolated by WorkerEnvironment; this change prevents Workers' lifecycle code from
counting/stopping each other's matches, not a malicious key holder.

Wrangler 4.149's `validateVars` explicitly warns that `vars` are not inherited; its schema/source
also treats environment bindings as non-inherited. Required shared values remain duplicated.
Provider-only values are omitted from the proving environment. This is intentional, not an
attempt to rely on inheritance that Wrangler does not implement.

## Local evidence

MEASURED final local results:

- Backend tests: 153 production-path tests and 93 EC2 tests pass in the shipped Worker runtime.
- Backend typecheck and generated boot embedding check pass.
- `python3 scripts/aws/test_harness.py`: 28 tests pass.
- `python3 scripts/aws/test_host_setup.py`: 20 tests pass, including key-only revocation and retry.
- Shellcheck passes for `scripts/aws/machine.sh`, `scripts/aws/host-setup` and `backend/ci-scope.sh`.
- Local esbuild Worker bundle and `git diff --check` pass. No Wrangler deploy command ran,
  including dry-run deploy; esbuild writes only ignored `backend/dist/worker.js`.
- No separate backend lint command is configured. No Rust changed and no cargo ran; the
  pre-commit hook checks only Rust formatting and is skipped for this commit.

Invariant tests deliberately exercise missing D1/Fleet tables and internal adapter exceptions;
those rejections are asserted and their runtime diagnostics are expected. Recognized transient
storage fixtures separately prove durable recovery. There are no unhandled test-run errors.

Primary sources: [systemctl machine-readable state and properties](https://www.freedesktop.org/software/systemd/man/latest/systemctl.html),
[Cloudflare DO error classification](https://developers.cloudflare.com/durable-objects/best-practices/error-handling/),
[Cloudflare D1 documented error causes](https://developers.cloudflare.com/d1/observability/debug-d1/),
[AWS optional Instance launchTime](https://docs.aws.amazon.com/AWSEC2/latest/APIReference/API_Instance.html),
[Ubuntu automatic updates](https://documentation.ubuntu.com/server/how-to/software/automatic-updates/),
and the pinned local Wrangler source. The real Ubuntu unit inventory and stop duration remain
unmeasured; no instance was launched to inspect them.

## Live proof — instructions only, not run

Every command below is for a separately authorized operator session, in a leased bay. Keep
identifiers and credentials in process memory; do not print/store them, use shell tracing, put
credentials in URLs or use a secrets file. No account spend figure is inferred here. Costs below
name metered resources rather than account-specific prices. Region is always `ap-southeast-1`.

0. **Read-only drain and standing verification, before deploy.** Count `Project=overmatch`
   runtime resources across all purposes and environments; every count must be zero. An untagged
   `Purpose=host` machine is invisible to this Worker's cap and sweep, as the named test proves:
   this is the reason for step 0. The standing security group is the explicit exception to zero;
   `verify` requires it, the keyless user and the policy. A literal zero of *all* tagged resources
   would contradict that prerequisite. `host-setup apply` must already have installed the standing
   parts in its separately authorized session; do not run it as part of this proof. From repo root:

   ```sh
   python3 - <<'PY'
   import sys
   sys.path.insert(0, "scripts/aws")
   import host_setup as h
   h.identity_account()
   resources = h.tagged_resources(project_only=True)
   for kind, rows in resources:
       if kind != "SecurityGroups":
           print(kind, len(rows))
   if any(rows for kind, rows in resources if kind != "SecurityGroups"):
       raise SystemExit("project resources remain across environments; proof refused")
   PY
   scripts/aws/host-setup verify
   ```

   Creates and changes nothing. Filtering only Project means a missing Purpose or environment
   tag cannot hide a runtime resource. `verify` must pass before proceeding.

1. **Deploy keylessly and configure Access.** From `backend/`:

   ```sh
   npm ci --ignore-scripts --no-audit --no-fund
   CI=true npx wrangler deploy --env aws-proving </dev/null
   CI=true npx wrangler d1 migrations apply DB --remote --env aws-proving </dev/null
   ```

   This creates/updates the proving Worker, scheduled sweep and separately named D1 database;
   Registry/Fleet objects materialize on use. It creates nothing on AWS. Leaving these deployed
   meters Worker/DO/D1 usage, including cron, but no instance compute. CI mode prevents tracked
   config writeback even when D1 is auto-provisioned. AWS secrets may remain absent.

   Before admin requests, add the proving hostname's `/admin` and `/admin/*` paths to the existing
   production Access application whose audience the proving config uses. Protect both paths.
   Alternatively use a separate app and explicitly supply its audience for the proving environment;
   never assume it equals production's. Verify an authenticated admin request succeeds and an
   unauthenticated one gets refused. Access configuration creates no AWS resource and no instance
   cost; its account plan/usage remains the operator's responsibility. Use the HTTPS proving origin.

2. **Issue the key only after the deployed Worker and Access checks.** From repo root, with pinned
   Wrangler on PATH:

   ```sh
   export PATH="$PWD/backend/node_modules/.bin:$PATH"
   scripts/aws/host-setup issue-key --env aws-proving
   scripts/aws/host-setup verify
   ```

   Creates one IAM access key and three Worker secret versions, without a disk secrets file;
   creates no machine. Standing IAM/network resources must already exist and be verified. The key
   itself has no instance runtime charge. From this point public joins can create up to this
   environment's authored cap of metered instances, volumes, public addresses and traffic; do not
   leave a keyed proving Worker unattended. Confirm the owner's existing billing alert; it is
   delayed evidence, not a guard. Do not request quota/support changes.

3. **Run exact-shape dry runs through Access.** POST `/admin/aws-proof` first with
   `{"version":"<published version>"}`, then with the current concrete image in `imageId`.
   Obtain that image with read-only `ssm get-parameter` using `--profile overmatch --region
   ap-southeast-1` and the parameter owned by `host_setup.py`; capture the value without printing.
   Both calls must return `proof=authorized, creates=false`. Every configured subnet must pass
   native and concrete-image authorization. A failure refuses this proof; do not widen IAM.
   Creates no slot or AWS resource; only ordinary Worker/API request usage remains. This endpoint
   stays for the cutover slice's repeat of the same verification.

4. **Run normal hosted play from a bay at the published version.** Set the test's `OVERMATCH_API`
   to the HTTPS proving origin and run the existing ignored
   `two_clients_join_the_same_hosted_match` test through `scripts/serial-cargo`, with features
   `dev_ui,bitprobe`. Its output includes an address: capture it through the existing harness
   sanitizer and suppress credential-bearing lines before any console/report output. Never print
   raw test output. Use the same sanitizer discipline for AWS reads. From repo root:

   ```sh
   set -o pipefail
   OVERMATCH_API="$proving_url" scripts/serial-cargo test --locked --lib \
     --features dev_ui,bitprobe two_clients_join_the_same_hosted_match \
     -- --ignored --nocapture 2>&1 |
   python3 -u -c '
   import re, sys
   sys.path.insert(0, "scripts/aws")
   import harness
   for line in sys.stdin:
       if re.search(r"secret|token|ticket|authorization|private.?key|netcode.?key", line, re.I):
           print("[credential line redacted]")
       else:
           line = re.sub(r"\b[0-9a-f]{64,}\b|\b(?:AKIA|ASIA)[A-Z0-9]{16}\b", "[credential]", line, flags=re.I)
           print(harness.scrub(line), end="")
   '
   ```

   This creates tickets, a Registry claim, a Fleet slot and one instance/root volume/interface/
   public address (a failed first boot can create a sequential replacement within the attempt
   budget). Both clients must join the same match. Leaving it running meters compute, disk,
   public address and traffic until termination. Successful play proves release fit on the explicit
   root, boot, reporting and the unchanged client contract. Cold-boot/WAN timings are observations.
   Record the observed boot-attempt count from `/admin`'s `boot N` state label before match end,
   including failed predecessors, and the observed boot duration against the configured start
   deadline. The updater stop duration and no-home server user remain unmeasured until this step
   passes. There is no live boot-attempt count from this lane.

5. **Forced silent boot recovers — kept offline, not claimed by step 4.** There is no safe live
   forcing operation in the present provider interface: user data is fixed, no SSH key is installed,
   and the Worker key has neither remote-execution nor network-edit permission. Changing subnets,
   the security group, image or a published archive would alter the verified launch contract or
   affect other machines/releases. An account-side HTTP blocking arrangement has not been verified
   here, so the report does not prescribe one as a proven safe procedure. No fault-injection flag
   or additional IAM permission was added. The shipped-runtime test holds termination pending,
   advances the configured start deadline, confirms no replacement yet, confirms termination, then
   observes replacement in the same location and exhaustion at the authored attempt budget,
   including late joiners. This offline step creates only synthetic machines and costs no AWS
   runtime. A real forced-silent recovery remains explicitly unverified before cutover.

6. **Attribute shutdown and prove the Fleet is empty.** Let clients leave and wait for empty exit,
   confirmed termination and a sweep. `/admin` must show no live/stopping servers, a successful
   recent sweep and **Fleet: 0 reservations**; zero EC2 instances alone cannot prove this.

   Keep the ids of this proof's instances in memory from a read-only `describe-instances` filtered
   by Project, Purpose and WorkerEnvironment, then read each id again after termination (the active
   listing intentionally excludes it). The AWS CLI call is `aws --profile overmatch --region
   ap-southeast-1 ec2 describe-instances --instance-ids <captured-id>` with a projection of only
   `Reservations[].Instances[].{state:State.Name,guard:StateReason.Code}`. Emit only those two fields,
   never the full response or id. `Client.InstanceInitiatedShutdown` witnesses the machine's own
   shutdown path; `Client.UserInitiatedShutdown` witnesses an API termination. A missing/other
   reason is inconclusive. An API-ended empty match proves Worker cleanup, not the privileged
   service-exit guard: record that distinction and leave the latter live proof open. This is
   read-only, creates nothing and adds only request usage. Read promptly while AWS retains the
   terminated record. No claim is made that the normal racing guards both win the same shutdown.

   A count/code-only check that includes recent terminated records (from repo root):

   ```sh
   python3 - <<'PY'
   import sys
   sys.path.insert(0, "scripts/aws")
   import host_setup as h
   h.identity_account()
   out = h.aws("ec2", "describe-instances", "--filters",
               "Name=tag:Project,Values=overmatch", "Name=tag:Purpose,Values=host",
               "Name=tag:WorkerEnvironment,Values=aws-proving")
   machines = [m for r in out["Reservations"] for m in r["Instances"]]
   ended = [m for m in machines if m["State"]["Name"] == "terminated"]
   if not ended:
       raise SystemExit("no ended proof machine visible; shutdown attribution is unproven")
   for m in ended:
       code = m.get("StateReason", {}).get("Code", "unavailable")
       allowed = {"Client.InstanceInitiatedShutdown", "Client.UserInitiatedShutdown"}
       print("terminated", code if code in allowed else "inconclusive")
   print("active", sum(m["State"]["Name"] != "terminated" for m in machines))
   PY
   ```

7. **Drain across environments; revoke only the key and Worker secrets.** Repeat step 0's
   count-only Project read; all runtime counts must be zero. Every previously keyed Worker's Fleet
   must also be empty. With pinned Wrangler on PATH:

   ```sh
   scripts/aws/host-setup revoke-key --env aws-proving
   scripts/aws/host-setup verify
   ```

   The new tested `revoke-key` subcommand refuses live project resources, revokes host-user keys,
   removes only the selected Worker's three hosting secrets, and proves both absent. The standing
   IAM user, inline policy and security group stay for the next slice. `teardown` would remove those
   parts and is deliberately not used. No key is created or machine terminated implicitly.
   Unrelated secrets survive; interrupted removal is restartable. The unkeyed proving Worker/D1
   retain history and their usage/storage; removing them remains a separate operator decision.
   Deployment can succeed again without secrets.

## Remaining live limits

Real IAM launch context/native SSM resolution, Worker-to-EC2 signing/fetch, GitHub release redirects,
filesystem fit, Ubuntu image unit inventory and actual systemd privilege/power-off behavior,
Access hostname/audience configuration, real end-to-end play and both shutdown attribution and
forced-silent recovery remain unmeasured here. The independent age rule is covered offline; no
24-hour live soak was run. No account price/quota claim or spend ceiling is made. The current key
can still be abused outside the Worker's cap; revocation does not itself clean retained resources.
