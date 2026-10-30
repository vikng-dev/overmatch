# Moving Overmatch's dedicated servers from Edgegap to Amazon GameLift Servers: feasibility scout

Read date for every source: **2026-10-30**. This is desk research only: no AWS account call was
made, nothing in the repo was edited, and the only AWS endpoint read was the public Price List.

**Labels**
- **SOURCE**: read from the cited primary source.
- **MEASURED**: measured by us, in the repo.
- **DERIVED**: computed here from SOURCE and MEASURED figures.
- **ASSUMED**: a judgement, e.g. an effort figure or a load shape.
- **UNCONFIRMED**: looked for in a primary source and not found.

**Prior art.** `.agents/scratch/hosting-2026-10/landscape.md` (2026-10-12) already ranked "GameLift
managed containers + Game Server Wrapper" first. `decisions.md` then shelved GameLift "as too heavy
for this stage (on file for launch)". This scout re-reads the primary sources and corrects one point
in that plan: a container fleet cannot be homed in Singapore (§2).

**Link keys**
- DG = `https://docs.aws.amazon.com/gameliftservers/latest/developerguide/`
- API = `https://docs.aws.amazon.com/gameliftservers/latest/apireference/`
- GR = https://docs.aws.amazon.com/general/latest/gr/gamelift.html (endpoints and quotas)
- FAQ = https://aws.amazon.com/gamelift/servers/faqs/
- GO = `https://github.com/amazon-gamelift/amazon-gamelift-servers-go-server-sdk/blob/dffb2a48/` (Go server SDK 5.6.0)
- WR = `https://github.com/amazon-gamelift/amazon-gamelift-servers-game-server-wrapper/blob/cc28e633/` (wrapper 1.2.0)
- PL = the GameLift Price List, publication 2026-10-13T15:30:44Z:
  `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonGameLift/20261013153044/<region>/index.json`

## Bottom line

1. **Integration: AWS's game server wrapper, in a managed container fleet.** The server needs no
   SDK. It reads its hosted config from an argument the wrapper fills per session, instead of from
   env. Effort is about **2 weeks of lane work in total (ASSUMED)**. About a day of that is the
   server; the rest is the Worker adapter, CI and a hands-on test. A native Rust client for the SDK
   protocol is the upgrade path if we ever need graceful termination or pre-booted processes. Do not
   bind the C++ SDK.
2. **Fleet shape:**
   - One multi-location container fleet per game version.
   - It is homed in Tokyo, because a container fleet cannot be homed in Singapore.
   - Locations: Singapore, plus Mumbai or Jakarta, as remote locations.
   - Instance: Graviton **c7g.large** On-Demand, with 2 game-server container groups per instance
     (one core per match).
   - A warm floor of one instance per hot location, and scale-to/from-zero everywhere else.
3. **Cost against Edgegap** (DERIVED; 30-player match ≈ 25.4 GB/match-h):

   | Volume | Edgegap | GameLift, 1 warm location | GameLift, 3 warm locations | GameLift, no warm floor |
   |---|---|---|---|---|
   | 100 match-h/month | **$261** | $79–90 | $213–223 | ~$11 |
   | 1,000 match-h/month | **$2,607** | $79–187 | $213–310 | ~$108 |

4. **Cold start:**
   - Warm: AWS says "seconds"; our boot is MEASURED at 0.6–2.3 s.
   - From zero: AWS publishes no number ("takes a little time"; "can take minutes"). It is UNMEASURED
     by us. The queue retries a placement for at most 600 s.
5. **Top risks:**
   - The free-bandwidth promise is 4½ months old, with no stated cap or end date. The whole cost case
     rests on it.
   - Cold-start time is unmeasured.
   - New-account instance quotas per remote location are unpublished.
   - Whether a Graviton core carries 30 tanks is unmeasured.
   - The wrapper's limits: a SIGKILL on terminate, and no player sessions.
   - A Linux-aarch64 server needs a bitprobe rerun under ADR-0028.

---

## 1. Running a Rust server on GameLift

**There is no Rust server SDK.**
- AWS's statement: "Libraries are available in the following languages: C++, C#, Go" (DG
  `reference-serversdk.html`), plus the Unreal and Unity plugins (DG `gamelift-supported.html`).
- No Rust server SDK exists in the `amazon-gamelift`, `aws`, `aws-samples` or `awslabs` orgs.
- The AWS-owned `aws-sdk-gamelift` crate is the *service API* client (CreateGameSession etc.), not
  the server SDK.
- A third-party `aws-gamelift-server-sdk-rs` 0.3.0 (MIT) exists. Whether it speaks SDK 5.x is
  UNCONFIRMED.

**What every game server must do.** Container fleets require a build "integrated with the server SDK
… (version 5.2 or greater)". The minimum calls are InitSdk, ProcessReady, OnStartGameSession,
OnProcessTerminate and ProcessEnding (DG `gamelift-roadmap-containers.html`).

### Option A: the game server wrapper (recommended)

**What it is**
- Repo: https://github.com/amazon-gamelift/amazon-gamelift-servers-game-server-wrapper. It is
  **Apache-2.0**, written in Go, and built on Go SDK 5.6.0.
- It "supports … Anywhere fleets, managed EC2 fleets, and managed container fleets" (WR `README.md:19`).
- Docs: DG `gamelift-wrapper-tutorial.html`.

**What it does**
- The wrapper is the process GameLift launches. It calls InitSDK and ProcessReady on one configured
  port.
- On CreateGameSession it calls ActivateGameSession, **then launches our binary** with templated
  command-line arguments: `{{.GameProperties}}` (a JSON string), `{{.GameSessionData}}`,
  `{{.GameSessionId}}`, `{{.IpAddress}}`, `{{.DNSName}}`, `{{.GamePort}}`, `{{.ContainerPort}}` and
  others (WR `README.md:576-586`, `441`; `src/pkg/hosting/gamelift/gamelift.go:276`).
- When our process exits, the wrapper calls ProcessEnding and exits (`gamelift.go:170-195`). Our
  server already exits after a match: `src/net/hosting.rs` posts `end` and exits on empty-timeout.
- The child inherits the wrapper's environment. **There are no per-session env vars**
  (`src/internal/multiplexgame/multiplexgame.go:223-236`).

**Limits**
- "does not support … customized server SDK callback functions and player session management"
  (WR `README.md:23`).
- No backfill. UpdateGameSession is ignored (`README.md:611-631`).
- On OnProcessTerminate it cancels the `exec.CommandContext` that owns our process. That is a
  **SIGKILL with no grace** (DERIVED from Go standard-library behaviour, untested). So a Spot notice
  or a scale-in never reaches our server.
- AWS's wording: "intended for evaluation and basic production use. Advanced features such as
  detailed player session management require a full server SDK integration" (DG
  `gamelift-wrapper-tutorial.html`). Also: "If your game doesn't need custom features, you can
  deploy … for production hosting" (DG `gamelift-supported.html`).

**What it costs us** (DERIVED from repo source)
- `HostedConfig::from_lookup(lookup)` in `src/net/hosting.rs` already parses the hosted contract
  through an injected lookup function.
- A lookup over a JSON map passed as one argument (`--hosted '{{.GameProperties}}'`, or
  `{{.GameSessionData}}`) is a small change. The `OVERMATCH_*` contract (`backend/contract/server-env.json`)
  stays the spec.
- **GameProperties limits:** 16 pairs, key ≤32 characters, value ≤96 (API
  `API_CreateGameSession.html`, `API_GameProperty.html`).
  - All 7 of our keys are ≤30 characters, and the netcode key and report secret are 64 hex each, so
    they fit.
  - `OVERMATCH_API` must stay ≤96.
  - GameSessionData (≤262,144 characters) removes every limit.
- **UDP 5888 stays fixed** inside the container. GameLift maps it to a connection port (§2), and the
  client already takes any port from the ticket (`backend/contract/ticket.ready.response.json`).

### Option B: a native Rust client of the SDK 5.x protocol (the upgrade path)

**The protocol is small, but documented only in code.**
- Transport: a WebSocket to `GAMELIFT_SDK_WEBSOCKET_URL`, with query parameters `pID`, `sdkVersion`,
  `sdkLanguage`, `ComputeId`, `FleetId` and `IdempotencyToken` (GO
  `server/internal/gamelift_manager.go:62-95`).
- Auth is either `Authorization=<token>` or SigV4-presigned query parameters. In containers the SigV4
  path uses the ECS task credentials (GO `server/server_state.go:136-191`,
  `server/internal/security/container_credentials_fetcher.go:21-39`).
- JSON envelope: `{Action, RequestId}`.
- **12 outbound actions:** ActivateServerProcess, HeartbeatServerProcess, ActivateGameSession,
  TerminateServerProcess (= ProcessEnding), AcceptPlayerSession, RemovePlayerSession,
  UpdatePlayerSessionCreationPolicy, DescribePlayerSessions, Start/StopMatchBackfill,
  GetComputeCertificate and GetFleetRoleCredentials.
- **4 inbound:** CreateGameSession (with GameProperties, GameSessionData, IpAddress, Port, DnsName and
  MaximumPlayerSessionCount), UpdateGameSession, TerminateProcess (`TerminationTime`) and
  RefreshConnection (GO `model/message/*`).
- **Heartbeat:** every 60 s ±10 s (GO `common/consts.go:21-24`). "doesn't respond for three
  consecutive health checks, then the service might shut down the process" (DG
  `gamelift-sdk-server-api.html`).
- **Not on any AWS doc page:** none names these actions or `GAMELIFT_SDK_WEBSOCKET_URL` (a search
  found nothing; UNCONFIRMED that none exists).

**Size and effort**
- The Go SDK core is MEASURED at 5,546 lines with comments. The WebSocket and auth internals are 1,519
  lines, of which 491 are SigV4 and container credentials.
- We need only the 5 required calls and the heartbeat. Estimate: **~800–1,500 lines of Rust and 1–2
  weeks (ASSUMED)**.

**What it buys over A**
- A graceful OnProcessTerminate, for the Spot notice and TerminateGameSession.
- A server pre-booted before the session, which saves our ~1–2 s boot.
- No Go process in the image.

**What it risks**
- An undocumented protocol that AWS may change.
- Whether the service accepts an unknown `sdkLanguage` is UNCONFIRMED; claiming "Go" sidesteps it.

**Trigger to take it:** Spot capacity, player-session validation, or sub-second session starts become
wanted.

### Option C: bind the C++ server SDK (not recommended)

It adds a C++ SDK build to the server on two architectures, plus FFI callbacks into the Bevy app. It
yields nothing beyond what B yields. Effort is ~1–2 weeks plus permanent build surface (ASSUMED). The
C++ SDK's own dependency list was not checked (UNCONFIRMED).

**A variant of A worth knowing.** AWS's containers starter kit (https://github.com/aws/amazon-gamelift-toolkit,
`containers-starter-kit`, Apache-2.0) runs a Go SDK *sidecar*:
- The game runs from container start and fetches session data from `localhost:8090/gamesessiondata`.
- When the game exits, the sidecar calls ProcessEnding.

It pre-boots the server, but the netcode key would then arrive after boot, which changes how hosted
mode starts. Not now.

## 2. Fleet shape

### Fleet type: managed containers

**Managed EC2 is worse for us:**
- It runs an uploaded file build on Amazon Linux 2023, not Docker.
- AL2023 ships **glibc 2.34** ([AL2023 release notes 2023.12.20260720](https://docs.aws.amazon.com/linux/al2023/release-notes/relnotes-2023.12.20260720.html)).
  Our server is linked on the CI runner's Ubuntu (`.github/actions/server-image/action.yml`) and
  needs `libasound2t64` and `libudev1` (`.github/actions/server-image/Dockerfile`). We would need a
  build against older glibc, or musl.
- With several processes per instance, every process gets identical launch Parameters and must pick
  its own port within `EC2InboundPermissions` (API `API_ServerProcess.html`, `API_CreateFleet.html`).
  Our port is a fixed 5888 (`src/net/server.rs:38`).

**Managed containers keep our image as it is:**
- Linux only, AL2023 hosts, the image in **ECR** (DG `containers-prepare-images.html`).
- The game listens on a container port from `ContainerPortRanges`. GameLift maps it to a connection
  port in `InstanceConnectionPortRange` (default from 4192) (API `API_CreateContainerFleet.html`;
  DG `containers-design-fleet.html`).
- `GameServerContainerGroupsPerInstance` is 1–5000, so set it to 2 on a c7g.large for one group per
  core.
- If the game server container exits, its group restarts, which gives a fresh wrapper for the next
  match (DG `containers-design-fleet.html`).

**Anywhere: no.**
- The hosts are ours, so their egress is ours, which defeats the point.
- It adds RegisterCompute, auth-token refresh and process launching (API
  `API_GetComputeAuthToken.html`; DG `integration-dev-iteration-agent.html`).

### Locations (SOURCE: DG `gamelift-regions.html`)

| Location | Code | Role |
|---|---|---|
| Tokyo | ap-northeast-1 | Home for a multi-location fleet |
| Seoul | ap-northeast-2 | Home for a multi-location fleet |
| Sydney | ap-southeast-2 | Home for a multi-location fleet |
| Singapore | ap-southeast-1 | Single-location home, or remote location |
| Mumbai | ap-south-1 | Single-location home, or remote location |
| Jakarta | ap-southeast-3 | Remote location only |
| Hong Kong | ap-east-1 | Remote location only |
| Osaka | ap-northeast-3 | Remote location only |
| Hyderabad | ap-south-2 | Remote location only |
| Malaysia | ap-southeast-5 | Remote location only |
| Thailand | ap-southeast-7 | Remote location only |

- Taipei and Melbourne are not listed.
- There are no Asian Local Zones.
- Opt-in Regions (e.g. Jakarta, Hong Kong, Thailand) must be enabled first.

**Container fleets:** "You can create a container fleet in any AWS Regions that Amazon GameLift
Servers supports for multi-location fleets" (API `API_CreateContainerFleet.html`). So in Asia the
home is **Tokyo, Seoul or Sydney**, with Singapore, Mumbai and Jakarta as remote locations.
- `DG containers-design-fleet.html` says "any AWS Region" instead. The API reference is trusted here
  (a docs conflict).
- landscape.md's test plan homed a container fleet in `ap-southeast-1`. Per the API reference, that
  fails.

**Instance types**
- A multi-location fleet's instance type must be available "in the home region and all remote
  locations" (API `API_CreateContainerFleet.html`).
- From PL SKUs (a proxy for availability, not a guarantee):
  - c6g, c7g, c7i and c8g are priced in all 11 APAC locations.
  - **c7a is priced only in Singapore and Tokyo.**
  - c8a is priced only in Tokyo.
  - c6a is missing in Seoul, Jakarta, Osaka, Malaysia and Thailand.
- **This makes Graviton the instance that spans Asia.** The x86 fallback that spans it is c7i, which
  has 2 threads per core, so a c7i.large is one core.

**GameLift On-Demand Linux, $/instance-hour (SOURCE: PL)**

| Location | c7g.large (2 Graviton3 cores) | c8g.large (2 Graviton4 cores) | c6a.large (1 Milan core, 2 threads) | c7a.large (2 Zen 4 cores) | c7i.large (1 core, 2 threads) |
|---|---|---|---|---|---|
| Singapore | 0.108 | 0.118 | 0.114 | 0.15023 | 0.132 |
| Tokyo | 0.117 | 0.128 | 0.124 | 0.163 | 0.143 |
| Seoul | 0.106 | 0.116 | — | — | 0.129 |
| Jakarta | 0.10681 | 0.11681 | — | — | 0.13033 |
| Mumbai | 0.067 | 0.073 | 0.064 | — | 0.115 |
| Sydney | 0.121 | 0.133 | 0.128 | — | 0.148 |

- Cores and threads come from the [EC2 compute instance specs](https://docs.aws.amazon.com/ec2/latest/instancetypes/co.html).
  c6a is the "AMD EPYC 7R13", with 2 threads per core.
- The c6a.large is the **same Milan generation as the EPYC 7713 we measured**, and it is one core.
  It is the like-for-like capacity check.
- Gen-6+ SKUs read "with free network bandwidth". For example: "$ 0.058 per c7g.medium GameLift Linux
  On Demand Instance Hour with free network bandwidth in Asia Pacific (Singapore)" (PL
  ap-southeast-1).

### One match per core

- MEASURED: one EPYC 7713 core saturates at 23–26 tanks, and 30 tanks exceeds that.
- At one core per match a c7g.large holds 2 matches. Whether a Graviton3/4 or Zen 4 core holds 30
  tanks is **UNMEASURED**. If a match needs 2 cores, density halves and compute doubles. The egress
  conclusion does not move.
- Server processes per instance are capped at 50, not adjustable (GR).

### Spot or On-Demand: On-Demand only, for now

- Spot "must use a queue" (DG `gamelift-compute.html`).
- AWS advises "at least one On-Demand fleet in each location" and "at least two Spot fleets with
  different instance types in each location" (DG `spot-tasks.html`).
- Reclaim comes with a "two-minute notification", possibly "before the warning arrives" (DG
  `fleets-spot.html`). Under the wrapper the notice ends in a SIGKILL.
- At our scale the burst compute that Spot would discount is ~$10–100/month (DERIVED, §4). Revisit at
  roughly 10+ concurrent matches (ASSUMED).

### Queues

- A queue (`TimeoutInSeconds` 10–600, default 600) fronts the version's fleet (API
  `API_CreateGameSessionQueue.html`).
- It allows up to 100 `FilterConfiguration.AllowedLocations` (API `API_FilterConfiguration.html`).
- It retries "until it reaches the queue's timeout setting". That is what carries a placement through
  a scale-from-zero.

## 3. Starting a match

### The call: `StartGameSessionPlacement` (recommended over `CreateGameSession`)

**StartGameSessionPlacement** (API `API_StartGameSessionPlacement.html`)
- `PlacementId` (≤48 characters, "unique across all Regions and cannot be reused") is our claim tag
  `om-<hex>`. `DescribeGameSessionPlacement(PlacementId)` is then exactly `Provider.findByTag`.
- `PriorityConfigurationOverride.LocationOrder = [<Worker's lowest-ping location>]` keeps the
  Worker's join rules in charge (API `API_PriorityConfigurationOverride.html`):
  - `PlacementFallbackStrategy=NONE` pins the location.
  - `DEFAULT_AFTER_SINGLE_PASS` lets a cold location fall back to a warm neighbour.
  - The override cannot be combined with `PlayerLatencies`.
- `GameProperties` and `GameSessionData` carry the `OVERMATCH_*` contract to the server (§1).
- **Result:** status PENDING → FULFILLED, TIMED_OUT, FAILED or CANCELLED, carrying `IpAddress`,
  `DnsName` and `Port` (API `API_GameSessionPlacement.html`).
- **How the result arrives:**
  - AWS says polling DescribeGameSessionPlacement "is not designed to be continually called … should
    only be used for games in development with low game session usage" (API
    `API_DescribeGameSessionPlacement.html`).
  - The production path is queue events, via an SNS topic or EventBridge (`PlacementFulfilled` carries
    `ipAddress`, `dnsName` and `port`) (DG `queue-notification.html`, `queue-events.html`).
  - An SNS HTTPS subscription to the Worker replaces Edgegap's `webhook_url`.

**CreateGameSession(FleetId, Location)** (API `API_CreateGameSession.html`)
- It is simpler and has an `IdempotencyToken`.
- With no free slot it fails with `FleetCapacityExceededException` ("Clients can retry"). The Worker
  would then own the retries through a scale-from-zero.

**Rate limits** (DG `api-limits-common.html`)

| API | Burst / rate (TPS) |
|---|---|
| StartGameSessionPlacement | 75 / 75 |
| DescribeGameSessionPlacement | 75 / 75 |
| CreateGameSession | 50 / 50 |
| DescribeGameSessions | 20 / 20 |

All of these are far above our load.

**Player sessions are optional** ("(Optional) Validate a new player", DG `gamelift-sdk-server-api.html`).

**Stop:** `TerminateGameSession` with `FORCE_TERMINATE` works on managed EC2 and container fleets.
`TRIGGER_ON_PROCESS_TERMINATE` needs the server to call ProcessEnding, which the wrapper effectively
does by killing our process (API `API_TerminateGameSession.html`). This is `Provider.stop` and the
orphan sweep's backstop.

### Signing from a Worker

- Endpoint: `gamelift.<home-region>.amazonaws.com` (GR), using AWS JSON 1.1 RPC
  (`X-Amz-Target: GameLift.<Action>`; botocore model
  https://github.com/boto/botocore/blob/develop/botocore/data/gamelift/2015-10-01/service-2.json).
- Requests are SigV4-signed by hand per https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html.
  It is HMAC-SHA256 and SHA-256, which WebCrypto in a Worker provides.
- **Least-privilege IAM** (https://docs.aws.amazon.com/service-authorization/latest/reference/list_gamelift.html):
  - `StartGameSessionPlacement` scopes to the queue ARN.
  - DescribeGameSessionPlacement, DescribeGameSessions, CreateGameSession and TerminateGameSession
    need `Resource: "*"`.
- The credential is a long-lived IAM access key held as a Worker secret (ASSUMED design).

### Address, port and our auth

- The client still gets `{address, port, key}` from `/v1/join/status`, unchanged.
- address/port: the placement's `IpAddress` and `Port`.
- key: still minted by the Worker's Registry and handed to the server in GameProperties.
- The netcode handshake, `protocol_id` and `server_addr_check: false` (`src/net/server.rs`) are
  untouched. GameLift's own player sessions are not used.
- **Visibility of the secrets:**
  - GameProperties and GameSessionData are readable through DescribeGameSessions and
    SearchGameSessions by any principal of *our* AWS account (API `API_SearchGameSessions.html`). That
    is the same trust boundary as Edgegap's hidden env vars.
  - The wrapper also puts them on our process's command line, which is visible inside the container
    (DERIVED).

### Placement time

| Case | Figure | Label |
|---|---|---|
| Warm (a process idles in ProcessReady) | "When game session slots are ready and waiting, it takes seconds" (DG `fleets-autoscaling-target.html`) | SOURCE |
| Our server's boot after the wrapper launches it | 0.6–2.3 s (`.agents/scratch/hosting-2026-10/risk-test.md`, on Edgegap) | MEASURED |
| Warm total | a few seconds, near Edgegap's warm 3.3 s | DERIVED |
| New instance | "It can take minutes to start up new instances and server processes" (same page); "Scale-out from zero takes time to provision and initialize instances" (DG `fleets_scale-to-from-zero.html`) | SOURCE |
| New instance, seconds | — | **UNCONFIRMED; no AWS figure found** |
| Fleet creation (for CI, not at request time) | a container fleet deployment "will take 5-6 minutes" ([blog 2025-03-10](https://aws.amazon.com/blogs/gametech/faster-multiplayer-hosting-with-containers-on-amazon-gamelift-servers/)) | SOURCE |

### Scale to zero: yes, per location

**What AWS ships**
- Shipped 2026-01-29 ([what's new](https://aws.amazon.com/about-aws/whats-new/2026/01/amazon-gamelift-servers-automatic-scaling/)).
- Set by `UpdateFleetCapacity`, with `ZeroCapacityStrategy=SCALE_TO_AND_FROM_ZERO` and
  `ScaleInAfterInactivityMinutes` 5–1440 (default 30) (API `API_UpdateFleetCapacity.html`).
- At zero, a session request triggers an immediate scale-out of one instance. Placement "may
  continue with other fleets or fleet locations", and the feature is "best paired with multi-location
  fleets and/or queues" (DG `fleets_scale-to-from-zero.html`).
- Under the default `MANUAL` strategy, GameLift "will not scale out from zero instances in response
  to game sessions" (API `API_ManagedCapacityConfiguration.html`).

**What a cold start costs (DERIVED)**
- In dollars, nothing: one c7g.large minute in Singapore is $0.0018.
- The cost is the first player's wait, which is **unmeasured**. ASSUMED: 2–5 min (EC2 boot, image
  pull, container start, wrapper ready, our 2 s boot).
- The Worker's `START_DEADLINE_S` is 60 (`backend/wrangler.jsonc`). It must rise for cold
  locations, or the claim must fall back to a warm neighbour.

## 4. Idle cost at realistic load

**Load**
- 30-player matches, 25.38 GB egress per match-hour (DERIVED in `hosting-costs-2026-10-30.md` from
  235 kB/s × 30).
- A few concurrent matches at peak, near zero off-peak (ASSUMED).

**Edgegap**
- Rates: $0.069/vCPU-h and $0.10/GB ([edgegap.com/pricing](https://edgegap.com/pricing); prior doc).
- Per match-hour: 25.38 GB × $0.10 + $0.069 = **$2.607 at 1 vCPU** ($2.676 at 2 vCPU). Edgegap is
  $0 when idle.

**GameLift**
- c7g.large On-Demand at 2 matches per instance (ASSUMED capacity, §2). Egress is $0.
- Floor: one always-warm instance per location (730 h).
- Burst bounds:
  - Lower: everything fits the floor.
  - Upper: every match-hour costs one extra instance-hour, a pessimistic bound for packing and
    scale-in lag (ASSUMED).

| Monthly, DERIVED | 100 match-h | 1,000 match-h |
|---|---|---|
| **Edgegap** (1 vCPU / 2 vCPU per match) | **$261** / $268 | **$2,607** / $2,676 |
| GameLift, Singapore warm (floor $78.84) | $79–90 | $79–187 |
| GameLift, Singapore + Tokyo + Mumbai warm (floor $213) | $213–223 | $213–310 |
| GameLift, Singapore + Tokyo + Sydney warm (floor $253) | $253–264 | $253–368 |
| GameLift, Singapore warm 8 h/day only (Worker-scheduled floor, $26) | $26–37 | $26–134 |
| GameLift, scale-to-zero everywhere (cold starts) | ~$11 | ~$108 |
| GameLift, Singapore warm on c7a.large (x86 fallback, floor $110) | $110–125 | $110–260 |

**Break-even (DERIVED):** one warm c7g.large in Singapore ($78.84/month) costs what Edgegap charges for
**~30 match-hours**. Above ~30 match-hours a month per warm location, GameLift is cheaper. At 1,000
match-hours it is **at least 8× cheaper** with 1–3 warm locations: the upper bounds are $187–310
against $2,607. The only scenario where Edgegap wins is
very low volume with several always-warm locations: 100 match-h against three warm floors is roughly
a draw.

**Minimum warm capacity**
- One instance (2 slots) in the location most players use. Every other location scales to/from zero,
  and its first match falls back to the warm neighbour through
  `PlacementFallbackStrategy=DEFAULT_AFTER_SINGLE_PASS`.
- A version that is no longer current goes to scale-to-zero, so release overlap does not double the
  floor.

**Not priced**
- ECR storage, CloudWatch logs and SNS: small, with figures not read (UNCONFIRMED).
- Public IPv4: §5.
- New AWS accounts' credits: per landscape.md, not re-verified.

## 5. Free bandwidth: the fine print

**What qualifies**
- "network bandwidth in and out of AWS at no additional charge for all instance types from
  generation 6 and later, including On-Demand and Spot … in all Amazon GameLift Servers supported
  regions, except China" ([what's new, 2026-06-15](https://aws.amazon.com/about-aws/whats-new/2026/06/amazon-gamelift-servers-free-network-bandwidth/)).
- FAQ: "Generation 6 and later, On-Demand and Spot instances in supported commercial … Regions,
  except China".
- FAQ: "All traffic between your game servers and connected players in and out of AWS is included at
  no additional charge".
- [Blog, 2026-06-16](https://aws.amazon.com/blogs/gametech/free-network-bandwidth-amazon-gamelift-servers-is-here-yes-really/):
  "across all Regions (excluding China), all Local Zones, Windows, Linux, Spot or On-Demand";
  "outbound network bandwidth from generation 6 (or newer) instances"; "Regardless of … number of
  connected players or server tick rate".

**Fleet types**
- **Managed containers:** not named on any page read. **DERIVED covered**:
  - Container fleets bill the same "GameLift EC2 Instance" SKUs (PL has no container SKUs).
  - The FAQ says "we do not bill based on how you upload your game build".
- **Anywhere:** not applicable. The hosts are ours and so is their egress (the FAQ's Anywhere
  billing covers only placements and connection minutes).
- **FleetIQ:** not mentioned. Treat it as excluded: "It does not apply to general AWS, EC2, EKS,
  resale, or non-game workloads" (FAQ).

**Locations:** GameLift in Local Zones supports only "C5d 2xlarge and R5d 2xlarge" (FAQ). Those are
generation 5, so in practice Local Zones get no free bandwidth (DERIVED; this contradicts the blog's
"all Local Zones"). Irrelevant in Asia, which has none.

**Cap or end date:** none stated on any page read. The nearest hedge is the instance-pricing page's
"At launch, free network bandwidth applies to…".

**Public IPv4: no charge found (UNCONFIRMED).**
- The GameLift Price List for ap-southeast-1 holds only two product families, "GameLift EC2
  Instance" and "GameLift Anywhere", with no IP-address or data-transfer SKUs (PL, read here).
- The FAQ and pricing pages never mention IPv4.

**Traffic to other AWS services, or to our Cloudflare Worker:** not defined. Heartbeats are
negligible anyway.

## 6. ARM (Graviton c7g/c8g)

**What GameLift needs**
- "Graviton Arm instances require a server build for a Linux AMI … Server SDK 5.0 or newer is
  required for Go" (DG `gamelift-compute.html`).
- Container fleets run AL2023, and an ARM image must be built for ARM (API
  `API_CreateContainerFleet.html`; DG `containers-build-fleet.html`).
- The image must be a single-architecture manifest (WR `README.md:402`).
- The wrapper is Go on SDK 5.6.0, so it is a `GOARCH=arm64` build.
- Nothing beyond an aarch64 build of the server, plus the wrapper, is required by GameLift.

**Our dependency tree** (MEASURED: `cargo tree --locked --offline --target aarch64-unknown-linux-gnu`)
- The native crates are `alsa-sys`, `libudev-sys`, `basis-universal-sys` (C++ via `cc`), `ring`,
  `wayland-sys`, `inotify-sys`, `renderdoc-sys` and `linux-raw-sys`. All build for aarch64 Linux.
  The two `-sys` crates that link system libraries need the arm64 `-dev` packages on the build host.
- There is no `target_arch`/`core::arch` code in `src/`. In `vendor/` the only `target_arch` gates are
  wasm32 ones.
- The whole tree already compiles for `aarch64-apple-darwin` in `release.yml`'s macOS job.
- No blocker found.

**CI**
- GitHub's arm64 Linux runners (`ubuntu-24.04-arm`) are free for public repos ([changelog 2025-08-07](https://github.blog/changelog/2025-08-07-arm64-hosted-runners-for-public-repositories-are-now-generally-available/)).
- So a native aarch64 `build-server` leg plus an arm64 Ubuntu image base is the change.

**Determinism: the one real item**
- ADR-0028 (`.agents/docs/adr/0028-cross-architecture-bit-determinism-via-glam-scalar-math.md`)
  made the sim bit-identical across architectures with `glam/scalar-math`. It is MEASURED identical
  for macOS-aarch64 against Linux-x86_64 over 3,072 ticks.
- A Linux-aarch64 server against x86_64 Windows and Linux clients is a new pair. The ADR's own rule
  ("compiler, or profile changes touching math or physics must rerun the macOS-aarch64/Linux-x86_64
  pair") implies a `scripts/bitprobe` run for Linux-aarch64 against Linux-x86_64 before shipping.

## 7. Migration shape

### What changes

**Release CI** (`release.yml` `register-edgegap`, `scripts/ci/edgegap-version.sh`)
1. Add an aarch64 `build-server` leg.
2. Add the wrapper binary (built from a pinned commit) and its `config.yaml` to the image.
3. Push the image to **ECR**, which GameLift requires. GHCR stays for Edgegap during a parallel run.
4. `CreateContainerGroupDefinition` for the version.
5. `CreateContainerFleet` (home Tokyo, plus locations).
6. Set the floor and scale-to-zero per location.
7. Point the version's queue at the fleet.
8. **Wait for ACTIVE (minutes) before `publish-release`.**
9. Scale old versions' fleets to zero, then delete them. The fleet quota is 20 per Region (GR); the
   FAQ says 10.

**Worker** (`backend/src/edgegap.ts` `Provider` is the seam; the Registry, D1, join rules,
heartbeat/end contract and `/admin` stay as they are)

| `Provider` method | GameLift call |
|---|---|
| `deploy` | `StartGameSessionPlacement` (`PlacementId` = tag, location override, `GameProperties`) |
| `findByTag` / `status` | `DescribeGameSessionPlacement` / `DescribeGameSessions` |
| `deployments` (sweep) | `DescribeGameSessions` per fleet location |
| `stop` | `TerminateGameSession FORCE_TERMINATE` |
| `locations` / `beacons` | `ListLocations` from a multi-location home Region |

- The beacons are a UDP echo at `gamelift-ping.<location>.api.aws:7770`, taking any non-empty payload
  ≤300 B, rate-limited to 3 TPS per sender address and port (DG `reference-udp-ping-beacons.html`).
  Unlike Edgegap, every deploy location has its own beacon.
- New: SigV4, an IAM user, the version → fleet/queue map, SNS HTTPS ingestion (or low-rate polling),
  and a longer start deadline for cold locations.

**Server**
- Read the hosted config from the wrapper's argument through `HostedConfig::from_lookup`.
- Nothing else: the port, the `end` POST and the exit on empty stay as they are.

### What stays the same

- The binary's sim and netcode.
- The client and its handshake.
- The Worker's API (`/v1/*`), the ticket flow, Registry state and history.
- Our auth: the per-match key minted by the Worker.

### Biggest risks

1. **The free-bandwidth policy.** It is the entire cost case. It is 4½ months old, has no stated cap
   or end date, and carries an "At launch" hedge. If it went away, GameLift egress would be priced
   like EC2 egress (~$0.12/GB in Singapore, prior doc), which is worse than Edgegap's $0.10.
2. **Cold start is unmeasured.** Either pay ~$50–90 a month per warm location, or accept multi-minute
   first joins or fallback to a neighbour. The hands-on test must measure it.
3. **New-account quotas.**
   - Instance limits per type and per remote location are unpublished, and vary by home-to-location
     pairing (API `API_DescribeEC2InstanceLimits.html`).
   - AWS asks for the launch questionnaire "at least three (3) months before conducting your first
     load test" (DG `gamelift_quickstart_customservers_checklist.html`).
4. **Capacity per core.** 30 tanks on one Graviton or Zen 4 core is unmeasured; a 7713 core saturates
   at 23–26 tanks. Two cores per match halve density.
5. **Wrapper limits.**
   - A SIGKILL on terminate or Spot reclaim, so our `end` POST is lost. The Worker's heartbeat
     deadline covers it.
   - No player sessions.
   - AWS calls it fit for "evaluation and basic production use".
   - The fallback, a native protocol client, rides an undocumented wire format.
6. **Cross-architecture determinism.** A Linux-aarch64 server needs the ADR-0028 bitprobe pair.
   Fallback: c7i (x86), at ~1.2× c7g per instance and half the cores per instance.
7. **AWS operational weight.** IAM, ECR, per-version fleets created in minutes, SNS verification and
   two docs conflicts (container-fleet home Region; fleet quota). This is what `decisions.md` called
   "too heavy for this stage".

### Next step, if pursued

Re-run landscape.md's one-hour GameLift test with two fixes:
- Home the container fleet in **Tokyo**, with Singapore remote.
- Use **c7g** (aarch64 image) and c6a.large as the like-for-like x86 control.

Record:
1. Placement to joinable, warm and from zero.
2. A 30-bot load on one c7g core.
3. Our handshake through the mapped port.

## UNCONFIRMED

- Scale-out-from-zero time, and placement-to-joinable time when warm. AWS gives no figures.
- Default instance quotas for a new account per type and remote location. Also the container-fleet
  quota, and whether the fleet limit is 20 (GR) or 10 (FAQ).
- Whether container fleets can be homed in Singapore or Mumbai. The API reference says no; DG
  `containers-design-fleet.html` implies yes.
- Free bandwidth:
  - Managed containers are covered only by derivation, not by name.
  - FleetIQ is not mentioned.
  - No cap or end date is stated anywhere.
- Any public-IPv4 charge on managed fleets. None was found in the price list or on the pricing pages.
- Actual instance availability per location; the §2 table uses Price List SKUs as a proxy.
- Whether the service accepts a non-C++/C#/Go `sdkLanguage`. That AWS documents the wire protocol
  nowhere (no page found).
- That the wrapper's terminate path SIGKILLs the child. This is from Go standard-library semantics,
  not tested.
- A Graviton3/4 or Zen 4 core's tank capacity at 64 Hz. The Linux-aarch64 against x86_64 bitprobe.
- ECR, CloudWatch and SNS costs (small, not priced). GameLift's billing granularity (per-second or
  per-hour).
