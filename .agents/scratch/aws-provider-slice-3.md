# Hosting slice 3: EC2 provider review closure

Subject: hosting for Overmatch, a multiplayer tank video game. Building lane `lane/aws-provider`,
bay-3. This revision made no deployment, key handoff, AWS request, quota request or external comment.
Only local checks and public primary-source reads ran. Production still selects Edgegap.

## Review dispositions

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
| Lens testing; security P3-10 | Fixed: fake never synthesizes an environment tag; owned fixtures specify `env.WORKER_ENV`. Untagged hosts remain invisible, explicitly tested and the reason for step 0. EC2 request locations read `hosting.location.city`. Other pre-existing coupled assertions remain in untouched tests. |
| Security P3-1 | Fixed step 0: Project-only count across all purposes/environments, zero runtime resources, then standing `verify`. Standing group cannot also be zero without contradicting verification. |
| Security P3-2 | Fixed: Registry alarms expire all live rows at the same provider-owned lifetime plus margin, independent of heartbeats. Expiry detaches/fails tickets before outside work, then confirms stop/release through durable retries. Heartbeats enforce expiry; placement excludes expired rows. |
| Security P3-4, P3-5 | Accepted live limits: updater stop latency and the no-home server user are unmeasured until step 4; it records boot-attempt count and duration. No invented live result. |
| Security P3-6 | Deferred by instruction: rate limits for `/v1/join` and `/v1/locations` remain an open cutover item. Distinct release versions can still drive GitHub reads and availability throttling. |
| Security P3-7 | Fixed: margin labelled Resolution for clock skew and boot time before the machine timer. Sweep and Registry read one `inventory.maxAgeMs` owner. |
| Security P3-9 | Fixed: restored main's unknown-outcome handling for non-ProviderError deploy exceptions, including null Edgegap JSON (regression test); no named failure justified rethrowing. Unsupported `/admin/aws-proof` returns 404 before Access/body checks, tested. |
| Security P3-11 | Fixed: added tested `revoke-key --env`: teardown removes standing parts and rotation creates a key, so neither does exactly what step 7 needs. Revocation retains user/policy/group for the next slice. |
| Security P3-12 | Fixed conservatively: stable-tag pattern excludes `+`, but the prerelease wildcard can admit it; upload-server uses raw tags. That proves the local name, not GitHub's asset name. EC2 refuses `+` with `releaseBuildMetadataUnsupported` before network/persistence. Edgegap's version syntax stays unchanged. |

The pre-existing Edgegap `findByTag` response-shape finding remains in its untouched file.
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

- Backend tests: 133 production-path tests and 88 EC2 tests pass in the shipped Worker runtime.
- Backend typecheck and generated boot embedding check pass.
- `python3 scripts/aws/test_harness.py`: 30 tests pass.
- `python3 scripts/aws/test_host_setup.py`: 20 tests pass, including key-only revocation and retry.
- Shellcheck passes for `scripts/aws/machine.sh` and `scripts/aws/host-setup`.
- Local esbuild Worker bundle and `git diff --check` pass. No Wrangler deploy command ran,
  including dry-run deploy; esbuild writes only ignored `backend/dist/worker.js`.
- No separate backend lint command is configured. No Rust changed and no cargo ran; the
  pre-commit hook checks only Rust formatting and is skipped for this commit.

The existing evidence test intentionally exercises a missing D1 table and emits its expected SQL
error; the test passes. Intentional Fleet failures are handled as typed results with no unhandled
test-run errors.

Primary sources: [systemctl machine-readable state and properties](https://www.freedesktop.org/software/systemd/man/latest/systemctl.html),
[Cloudflare DO error classification](https://developers.cloudflare.com/durable-objects/best-practices/error-handling/),
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
