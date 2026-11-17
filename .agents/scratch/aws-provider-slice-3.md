# Hosting slice 3: EC2 provider error-rule closure

Subject: hosting for Overmatch, a multiplayer tank video game. Building lane `lane/aws-provider`,
bay-3. This revision made no deployment, key handoff, AWS request, quota request or external comment.
Only local checks and public primary-source reads ran. Production still selects Edgegap.

## Live proof, first attempt

MEASURED from the refused launch's CloudTrail record and decoded authorization message: the
Worker's real `RunInstances` failed with `UnauthorizedOperation` on the image resource, with
"no identity-based policy allows". EC2's image context held `ec2:Owner = amazon`,
`ec2:Public = true`, `ec2:ImageType = machine` and `ec2:RootDeviceType = ebs`.
`DescribeImages` for the SSM-resolved Canonical Ubuntu image reported `ImageOwnerAlias: amazon`
and a numeric `OwnerId`. The policy used that numeric id for `ec2:Owner`; the simulation supplied
the same numeric context and treated `amazon` as a foreign owner. DERIVED: that owner condition
cannot match the observed EC2 context. The other launch resources were not reached; their
authorization remains unproven, not proven wrong.

The refused request carried `blockDeviceMapping`, `clientToken`, `disableApiStop`,
`disableApiTermination`, `instanceInitiatedShutdownBehavior`, `instanceMarketOptions`,
`instanceType`, `instancesSet`, `metadataOptions`, `monitoring`, `networkInterfaceSet`,
`tagSpecificationSet`, `tenancy` and `userData`.

## Error-rule closure

Decision: use one rule across every Worker catch in the lane's diff. Parse outside failures once
at their native I/O, JSON, response-schema or persisted-cache edge; catch only those typed errors
for refusal, a safe cause log, durable retry or unknown-outcome reconciliation. An unexpected
exception propagates and fails the invocation. Native response-stream errors have no stable
platform subtype: their small, I/O-only broad catch states that reason beside it. The batch runner
collects all task failures and rethrows them. HTTP entry handles only typed request, configuration
and platform refusals; scheduled entry handles only typed configuration refusal. Bugs still reject.

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

### Worker input-edge closure

Decision: parse the selected provider and all consumed operator settings in `config(env)` before
routing or scheduled work. Cache the checked `Config` by immutable Worker environment; Registry
construction and Fleet's provider path use the same parser in their own isolates. Consumers retain
URLs, headers, numbers, the network object and the checked provider variant rather than parsing
raw settings at use sites. A `ConfigurationError` names only the setting and its required form,
never its value; HTTP logs it and returns 503, cron logs it and refuses that run. The cache reuse
is tested for numeric/URL/header/enum settings and separately for EC2 network JSON.

Keyless EC2 startup remains valid: absent keys/network are explicit nullable configuration state;
operations needing them retain their typed `credentials`/`config` refusal before admission or an
AWS exchange. Missing Access configuration still disables admin with 403. Unselected provider
settings are unused, so EC2 does not require Edgegap's vars or key. No gameplay, placement, retry,
launch policy or timing choice changed. Foreground RPC classification adds no new deadline.
Malformed optional request fields now refuse instead of pretending to be absent; an omitted end
reason still means the protocol's ordinary exit, and an omitted webhook id remains a hint without
an adoptable deployment id. Request ids and operator regexes must consume the entire input,
including a terminal newline. Provider deployment ids must be well-formed Unicode before URL
encoding. Incoming request streams and provider response streams each isolate their native I/O
catch because their platform errors have no stable subtype.

All four findings in `lenses-provider-4.md` are closed, including both pre-existing ones. The
Edgegap `deploy`/`beacons` intermediates are removed (DERIVED: two lines saved). The durable Fleet
release test scopes a transient storage fault around execution of the real prototype method and
restores it after that call; RPC does not expose instance-owned method overrides. It matches no SQL
text. The other pre-existing testing P3s at the original `evidence.spec.ts:240` and
`ec2.spec.ts:235,511` remain by instruction.

The sweep also closed raw HTTP Registry RPC transport failures, history's broad D1 catch, native
Access JWK import refusals and invalid forwarded client addresses. D1 outages use the documented
infrastructure allowlist; missing tables/constraints still reject. Access keeps jose's JWT/JWKS
parser and cache; native key-import failures are classified only around its untrusted-key resolver,
and small RSA keys refuse before jose's verifier can throw an untyped error.

### Complete input-edge table

Scope: every consumed HTTP field/header, parsed operator setting, external fetch, D1 operation,
Durable Object RPC and local Durable Object storage operation in `backend/src/`. Generated boot
code is an embedded view of the separately tested machine script, not another Worker entry point.
Names below are functions rather than unstable source line numbers. Cloudflare constructs the
incoming `Request` (including URL/header syntax); Worker-owned SQL rows and RPC return shapes
are internal schema/interface invariants, not untrusted provider JSON.

| Input / every call site | Parse once / checked type | Typed, visible refusal or named absence |
|---|---|---|
| Incoming URL, method and route; `/v1/locations?version=` (the only consumed query parameter) | `route`: one `URL`, route regexes; `validVersion` checks the whole version | Bad version: `RequestError(fields)`, logged 400; unknown route/method: 404. Origin is the canonical `URL.origin`; the existing EC2 HTTPS gate returns 400. Other query parameters are unused. |
| Body stream at join, ticket status, heartbeat, end, webhook and AWS proof | `body`: native text read once, JSON parsed once to `unknown`; already consumed/locked bodies are internal bugs | Transport: `RequestError(bodyRead)`; syntax: `RequestError(json)`; both logged 400, no payload logged. Internal bugs propagate. |
| Join `version`, `locations`, `pings` | `parseJoin` produces `JoinRequest`; bounded location list, finite nonnegative ping map; absent estimates mean no estimate | `RequestError(fields)`, logged 400; unknown catalog locations: existing 400. |
| Ticket-status `ticket` | String plus `versionOf` checked versioned id | Malformed/absent/unknown ticket: 404; no credential in URL/log. |
| Heartbeat/end server-id path; webhook server-id path and nonce | `pathSegment` decodes once, `versionOf` checks id; route regex checks nonce | `URIError` becomes logged `RequestError(pathEncoding)`/400; malformed/unknown id: 410 for reports, 404 for webhook; wrong nonce: 404. |
| Heartbeat `players`, `capacity`, `state` | `parseHeartbeat` produces `HeartbeatRequest` with integer counts and state union | `RequestError(fields)`, logged 400. |
| End `reason`, `crash` | Object and optional text checked before `Registry.end`; reason bounded as before | Wrong object/field types: logged 400; omitted reason: protocol exit; omitted crash: ordinary non-crash report. |
| Webhook `request_id` | Optional whole-string provider-id syntax to `string | null` | Invalid supplied id/object: logged 400; omitted id: existing status hint, no adoption from a missing id. |
| AWS proof `version`, `imageId` | Whole-string version plus optional AMI syntax before `dryRun` | Invalid fields: logged 400; Access: 403; unsupported provider tool: 404 before Access/body; provider refusal: logged 503. |
| `authorization` on heartbeat/end | `bearer` parses once to report secret; empty means missing/non-Bearer credential | Registry authenticates once against its stored hash: 401 for bad secret, 410 for unknown/ended server. |
| `cf-connecting-ip` on join | `clientIp` checks IPv4 octets or native IPv6 URL syntax; keeps admitted text | `RequestError(clientIp)`, logged 400 before provider work; absent header: explicit local/test request state. |
| `cf-access-jwt-assertion` on admin/proof | `accessDenied`: jose `jwtVerify`, signature, issuer, audience and required expiry; verified claims are not used as unchecked data | Missing/invalid/expired token: 403; parsed JOSE errors logged by code only. JWKS outages/malformed keys are typed and visibly refuse. |
| `PROVIDER`; `LOCATION_HINT` | `config`: installed factory key to provider variant; platform location-hint enum (including both APAC subregions) | Named `ConfigurationError`, logged 503 / logged scheduled refusal. Empty location hint deliberately requests platform placement. |
| `EDGEGAP_API`; `EDGEGAP_TOKEN` | Selected-provider `Config`: checked HTTP(S) `URL` without credentials; authorization `Headers` constructed once | Malformed URL/header or absent token: named `ConfigurationError`, logged 503 / scheduled refusal, value never logged. |
| `JOIN_ATTEMPTS`; `USD_PER_VCPU_MINUTE` | Selected Edgegap config: finite numbers with existing minima | Named `ConfigurationError`, logged 503 / scheduled refusal. |
| `WORKER_ENV`; `AWS_HOST_CONFIG` | Selected EC2 config: whole-string deployment identity; JSON parsed once to checked `Network` (group id, nonempty unique subnet ids, exact keys) | Malformed supplied value: named `ConfigurationError`, logged 503 / scheduled refusal. Missing network is explicit keyless config; launch/proof refuses before reservation/exchange. |
| `AWS_ACCESS_KEY_ID`; `AWS_SECRET_ACCESS_KEY` | EC2 config retains optional key strings, validating the access-key header representation once; signing consumes these checked settings | Missing keys: explicit keyless state and typed `ProviderError(credentials)` on an operation needing them. Invalid access-key header: named `ConfigurationError`, logged 503 / scheduled refusal. |
| `ACCESS_TEAM_DOMAIN`; `ACCESS_AUD` | `config`: checked issuer URL and derived cert URL; audience is an opaque string, not JSON/regex | Malformed supplied URL: named `ConfigurationError`, logged 503 / scheduled refusal; missing team/audience: disabled admin, 403. |
| `CAPACITY`, `HEARTBEAT_INTERVAL_S`, `HEARTBEAT_DEADLINE_S`, `START_DEADLINE_S`, `STATUS_POLL_S`, `RESERVATION_TTL_S`, `TICKET_TTL_S`, `EMPTY_EXIT_S`, `SERVER_STATS_S`, `SERVER_LOG_RETENTION_DAYS`, `ENDED_RETENTION_S`, `LOCATIONS_CACHE_S`, `PROVIDER_TIMEOUT_S` | `config` produces finite numbers with existing minima and unit conversion; converted overflow also refused | Named `ConfigurationError`, logged 503 / scheduled refusal. No raw value appears in diagnostics. |
| `EDGEGAP_APP`; `EDGEGAP_PORT_NAME`; remaining binding/token reads | App/port are required nonempty strings in the admitted provider variant; `DB`, `REGISTRY`, `FLEET` are platform handles; evidence uses token only for literal replacement, not regex construction | Missing/wrong-type strings: named ConfigurationError, logged 503 / scheduled refusal. Binding/SQL invariants remain bugs; transport handled as below. No other env value is parsed as URL/number/JSON/regex. |
| All Edgegap fetches: deploy, status, stop, logs, find-by-tag, paginated deployment/location inventory, beacons | `request` -> `providerFetch`; body read/UTF-8 bounded by `providerText`; `outsideJson` then operation schema to provider types | `ProviderError` for native network/stream/decode/syntax/schema/status/deadline failure; visible HTTP refusal, safe cause log and existing durable reconciliation/retry. No malformed inventory proves absence. |
| All EC2 fetches: RunInstances (including proof), DescribeInstances, TerminateInstances | `query` -> `providerFetch`/bounded text -> `parseXml`; `Xml.one/optional` cardinality and operation field checks | `ProviderError`: classified code/state/unknown result; DryRunAccepted only confirms proof; eventual absence stays unknown until its deadline. Existing logs/refusals/reconciliation retained. Launch time is checked timestamp or named `unreadable`. |
| Release fetches: checksum GET, archive HEAD | `serverRelease`: bounded native fetch/text, exact checksum line and filename to `ServerRelease` | Typed release missing/unavailable/refused, visible 400/404/503 or ticket reason; body cancellation uses the typed stream edge. |
| Access cert fetch and key import | `providerFetch` for transport; jose parses JWKS once per refresh into cached keys; native import errors classified at resolver, RSA size checked before verify | `ProviderError(accessKeys)` or JOSE error, safe code log and 403; unexpected application exceptions propagate. |
| D1 admin: versions, totals, recent sessions | Each statement through `d1Call`, schema-owned result types | Documented transient faults/deadlines: `ProviderError`, logged HTTP 503; SQL/schema bugs propagate. Aggregate-without-row is a bug. |
| D1 history: requested/deployed/ready/crashed/ended writes | `recordEvent` -> `d1Call`; statements bind typed `SessionEvent` | Typed infrastructure failure logged as best-effort history failure; SQL/schema bugs propagate. |
| D1 evidence: first crash, capture marker/read/update, failure record, retention batch | `recordCrash`, `stopWithEvidence`, `expireServerLogs` -> `d1Call`; bounded sanitized text / schema-owned rows | Typed faults log cause; capture failures still stop, recorded marker prevents repeated capture, later sweep retries retention; SQL bugs propagate. |
| D1 sweep state: write/upsert and last-sweep read | `save`/`lastSweep` -> `d1Call`, `SweepResult` schema | Typed fault logged; save retried by later sweep; dashboard shows no available snapshot on read outage; no row before first sweep is legitimate absence. Bugs propagate. |
| HTTP Registry RPC: locations, join, ticket, heartbeat, end, webhook, including `getByName` | `platformCall` classifies documented retryable/overloaded flags; Worker-owned typed RPC interfaces | Typed transport refusal logged with HTTP 503. Foreground operation lifetime unchanged. Remote invariant exceptions propagate. |
| Admin Registry snapshots; sweep `liveDeployments` and `claimExists` RPC, including `getByName` | `outsideRpc`: documented flags/deadlines to `ProviderError`; internal interface types retained | Dashboard logs/shows unavailable snapshot; sweep logs/counts failure and leaves ownership undecided for retry. Remote bugs propagate. |
| Fleet RPC: reserve, reservations, release, including `getByName` | `fleetCall`/`outsideRpc`; expected remote failures cross RPC as `FleetResult` data, not custom exception properties | Typed transport/storage/provider failure; admission/refusal or logged durable stopping-row retry. SQL/schema bugs propagate. |
| Fleet local SQL: schema creation, insert, select, delete | Internal schema; storage callbacks classify native reset/overload flags as `FleetResult(storage)`; constructor faults retain platform flags at RPC edge | Logged typed failure, never false release success; caller retains retry state. SQL/schema errors reject. |
| Registry local SQL and alarm get/set/delete | Internal schema-owned rows/queries; native platform flags retain their meaning at the classified RPC edge; alarm invocation is platform-owned | HTTP/admin/sweep callers visibly refuse typed transport faults. A failed alarm rejects to Cloudflare's logged retry path; invariant errors also reject rather than disappearing. |
| Persisted provider catalog/release JSON | `cachedCatalog`/`cachedRelease`: outside JSON and shape parser at each cache read | `ProviderError` visible refusal; expired-but-valid catalog may serve stale only with a logged provider failure. App-owned ticket arrays/maps were checked before persistence; corrupt internal rows are bugs. |

`Origin` is not read from a request header: callbacks use the parsed request URL's canonical origin.
No setting is compiled as an operator-provided regex; regexes check only the identity/network
settings listed above. Unknown request headers/fields/query parameters are unused. No additional
Worker fetch/D1/RPC entry was found by source search.

### Complete catch audit

Every Worker source catch was re-read, including unchanged files. These owners cover the catches
without duplicating the input inventory above:

| Owner / catches | Deliberate outcome |
|---|---|
| `config`: text, URL, header, network JSON | Native parse refusal -> named ConfigurationError; unexpected exceptions propagate. |
| `index`: body stream, JSON, path decoding, IPv6, proof, HTTP entry, scheduled entry | Native request parsing -> RequestError/400; proof catches ProviderError/503; HTTP handles only RequestError, ConfigurationError, ProviderError; cron only ConfigurationError. Bugs propagate. |
| `provider`: fetch, outside JSON, foreground/bounded RPC, D1, UTF-8, native response read/cancel | Each native edge classifies only its documented failures; stream-only broad catch has no application callback. All other exceptions propagate. |
| `admin`: JWK resolver, JWT verification, Registry snapshot | Outside key import -> ProviderError; JOSE/ProviderError -> visible Access refusal; snapshot ProviderError -> logged unavailable. Bugs propagate. |
| `history`: event write | ProviderError logged; bugs propagate. |
| `fleet`: inventory, storage | ProviderError / documented platform flags -> logged FleetResult; bugs propagate. |
| `registry`: locations/join catalog, release checks, webhook, alarm task, cache refresh, deploy/adopt/poll/stop/release | Only ProviderError drives visible refusal, logged stale-cache use, or existing durable retry/reconciliation. Release cache admits only releaseMissing as a negative entry. All other errors propagate. |
| `sweep`: listing, per-host status, Registry ownership, stop, slot cleanup/list, state read/write | Only ProviderError logged/counts a deferred decision; bugs propagate, including AggregateError after concurrent work. |
| `ec2`: query/deploy deadlines, launch retries, proof, status/stop absence | Only DeadlineError, ProviderError, DryRunAccepted or MissingInstance as appropriate; bugs propagate. |
| `edgegap`, `release`: bounded exchanges | Only DeadlineError translated; response handlers' bugs propagate. |
| `evidence`: capture, error record, retention | Only ProviderError logged with explicit stop/retry disposition; bugs propagate. |
| `ids`: base64url/UTF-8 decoding | Native InvalidCharacterError/TypeError -> malformed id, visibly refused by caller; other errors propagate. |
| `deadline`: concurrent task collection | Completes independent tasks and propagates every rejection in AggregateError. |

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

**The image-owner correction changes the rendered policy; standing IAM needs reconciliation
before repeating the live proof.** The policy template has no `aws:TagKeys` allowlist and permits
extra creation tags. Its RequestTag conditions require Project/Purpose; CreateTags is restricted
to RunInstances. Existing simulation cases still test that contract, and `test_host_setup.py`
passes. No AWS action was added.

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

### Worker value-domain closure

All four error-handling P2s, the brevity P3 and both pre-existing testing P2s in
`lenses-provider-5.md` are closed. Authenticated heartbeat counts are integers: capacity uses the
server's netcode ceiling, and players are between zero and that heartbeat's capacity. Refusal is
`RequestError` at HTTP admission, before Registry SQL. The arrival `LIMIT` cannot receive an
unbounded JavaScript number. Join pings use the shipped client's whole-millisecond `u32` domain;
their keys use the same Unicode id domain as selected locations, and both collections use the
selection budget. Non-finite, fractional, negative and overflowing request numbers refuse.

Decision: `SERVER_SETTING_DOMAINS` in `backend/src/config.ts` is the one Worker table for all
forwarded numeric settings. Rust owns the domains: capacity and hosted durations in
`src/net/hosting.rs`, stats periods in `src/net/server_stats.rs`. There is no shared data source
across the Rust and TypeScript parsers, and this lane must not edit Rust. The table gives the Rust
line references, and the boundary tests read the table rather than copying its bounds. Existing
stricter Worker minima for heartbeat and stats stay unchanged. Rust `Duration`'s representable
range supplies the stats upper bound (DERIVED: floating-point seconds must be less than 2^64).
The generated server id, API origin, key and report token already satisfy the hosted string
parsers; no operator-controlled string is forwarded as one of those generated credentials.
Heartbeat/start deadlines are Worker-only settings, not parsed or received by the game server;
their existing finite, positive seconds and finite conversion checks remain. All other consumed
numeric operator settings retain their minima and finite-conversion checks, with decimal syntax
admission; empty, nonnumeric, non-finite and overflowing representations are configuration errors.

Disabled Access or keyless hosting now requires an absent setting. Supplied empty, whitespace-only,
control-character, ill-formed Unicode or wrong-shaped optional settings are named
`ConfigurationError`s. Access validates the supplied URL before normalization, even if the audience
is absent, and requires an HTTP(S) origin without credentials, path, query or fragment. AWS keys
are visible ASCII tokens without spaces; network JSON retains its exact security-group/subnet
shape and uniqueness gate. Unselected provider settings remain unused. Optional Edgegap retry
and price settings are required when that provider is selected; malformed or missing values
already refuse, and numeric admission now also tests empty and non-decimal representations.

The string request sweep retained version/id/nonce/image/request-id grammars, native IP parsing
and the existing 404/410/401 outcomes for unknown identifiers or credentials. Location ids and
diagnostics now also reject ill-formed Unicode. Diagnostic text remains free-form (empty is
allowed), with the existing short reason and redacted UTF-8 tail retention owned by history and
evidence. The server state remains its three-value enum. No placement, launch, retry, lifecycle
or retention policy changed.

`providerName` is deleted; its two callers project the admitted provider directly. The failed-ticket
format test reads the configured startup deadline, and the adapter forwarding test reads the
configured application name. Tests witness rejected heartbeat inputs leaving the real Registry
snapshot unchanged, accepted integer endpoints reaching it, ping endpoints reaching placement,
and malformed optional settings refusing even when another setting leaves the capability absent.

Operational exception: an accidental source search printed configuration identifiers from generated
bindings. It read no credential files. Subsequent searches excluded generated bindings. No account
command or live AWS request ran.

## Local evidence

MEASURED final local results:

- Backend tests: 246 production-path tests and 100 EC2 tests pass in the shipped Worker runtime.
- Backend typecheck and generated boot embedding check pass.
- `python3 scripts/aws/test_harness.py`: 28 tests pass.
- `python3 scripts/aws/test_host_setup.py`: 20 tests pass, including key-only revocation and retry.
- Shellcheck passes for `scripts/aws/machine.sh`, `scripts/aws/host-setup` and `backend/ci-scope.sh`.
- Local esbuild Worker bundle and `git diff --check` pass. No Wrangler deploy command ran,
  including dry-run deploy; only the local `wrangler types` ran for typecheck. Esbuild writes only
  ignored `backend/dist/worker.js`.
- No separate backend lint command is configured. No Rust changed and no cargo ran; the
  pre-commit hook checks only Rust formatting and is skipped for this commit.

Initial focused reproduction: 18 new input-edge cases failed while the existing 20 error tests
passed. Final regression coverage exercises interrupted streams, malformed encodings, all numeric
settings, configuration cache reuse, malformed keys, whole-string regexes, optional fields and
HTTP RPC failures.

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
   Wrangler on PATH and the native authentication exports described in
   [`host-setup.md`](../../scripts/aws/host-setup.md). For the operator shell that already exports
   `CLOUDFLARE_GLOBAL_KEY` and `CLOUDFLARE_EMAIL`, prepare Wrangler's key name without printing it:

   ```sh
   set +x
   unset CLOUDFLARE_API_TOKEN
   export CLOUDFLARE_API_KEY="${CLOUDFLARE_GLOBAL_KEY:?export the account key first}"
   export CLOUDFLARE_EMAIL
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

Successful full-request IAM authorization/native SSM resolution, GitHub release redirects,
filesystem fit, Ubuntu image unit inventory and actual systemd privilege/power-off behavior,
Access hostname/audience configuration, real end-to-end play and both shutdown attribution and
forced-silent recovery remain unmeasured here. The independent age rule is covered offline; no
24-hour live soak was run. No account price/quota claim or spend ceiling is made. The current key
can still be abused outside the Worker's cap; revocation does not itself clean retained resources.

## Live proof, second attempt (2026-11-17 UTC, alpha.20, proving Worker)

MEASURED, one launch, after the image-owner fix was applied with `host-setup apply`:

- Step 2: `issue-key --env aws-proving` installed the pair; `verify` passed.
- Step 4: the first join launched one machine at the first boot attempt. The Worker recorded ready
  31 s after the request (start deadline 240 s). Both clients received tickets for the same server,
  connected, replicated both tanks and received 64 ticks/s with 0 holds and 0 cuts for 90 s, in two
  runs against the same machine. Peak players recorded: 2.
- The test itself failed both runs with `in match: 0, tanks seen: 2`: its client app never mounts
  `latch_tank_seen`, so it cannot reach `Flow::InMatch` against any server. That is a defect of the
  test's hand-copied client wiring, not of hosting; the test's assertion of normal hosted play is
  therefore still open.
- Step 6: the server reported `empty for 300 s`; the Worker ended the session and the machine was
  terminated by the API (`Client.UserInitiatedShutdown`). The on-machine power-off guard was not
  witnessed. The report arrived 8.7 min after the last client process exited, against an expected
  5 min plus the link timeout: unexplained, and the machine's journal was not captured (no
  `server_logs` row), so it cannot be settled from this run.
- Step 7: instances, volumes and interfaces counted 0; `revoke-key --env aws-proving` and `verify`
  passed; the host user holds 0 keys. The keyless proving Worker's scheduled sweep now records
  `list: credentials … refused` on every run, which is its correct answer without a key.
- Not done: step 3's `/admin/aws-proof` dry runs and the `/admin` Fleet-empty reading (both need the
  owner's Access login); a forced silent boot (step 5).
