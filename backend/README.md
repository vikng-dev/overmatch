# Overmatch matchmaking Worker

A Cloudflare Worker that finds or starts an Edgegap game server for a client's version and region
choice, hands the client its address and netcode key, and tracks servers through their heartbeats.
Design and decisions: `.agents/scratch/hosting-2026-10/` (`decisions.md`, `state-store.md`,
`provider-revisit.md`, `risk-test.md`).

## Layout

| Path | What it holds |
|---|---|
| `src/index.ts` | HTTP routing and request validation |
| `src/registry.ts` | `Registry` Durable Object, one per game version: servers, tickets, alarm |
| `src/edgegap.ts` | The provider adapter; the only Edgegap-specific code |
| `src/history.ts` | `recordEvent`, the single writer of D1 `sessions` |
| `src/admin.ts` | `/admin` dashboard and its Access JWT check |
| `src/types.ts` | The `/v1` wire types |
| `contract/*.json` | One example per request and response; `test/contract.spec.ts` holds them to the handlers |
| `migrations/` | D1 schema |

## API (`/v1`, JSON)

| Method and path | Caller | Auth | Answer |
|---|---|---|---|
| `GET /v1/locations?version=V` | client | none | Continents → locations, each with a same-city beacon or `null` and the live match count for V |
| `POST /v1/join` | client | none | `{ticket}`. Body: `{version, locations, pings}` |
| `POST /v1/join/status` | client | the ticket, in the body `{ticket}` | `{state: starting\|ready\|failed, address?, port?, key?, reason?}`; poll until not `starting` |
| `POST /v1/servers/{id}/heartbeat` | game server | `Authorization: Bearer <OVERMATCH_REPORT_SECRET>` | 204; **410 = unknown server, exit now**; 401 bad secret |
| `POST /v1/servers/{id}/end` | game server | same | 204 (the Worker stops the deployment); 410 unknown |
| `POST /v1/edgegap/webhook/{id}/{nonce}` | Edgegap | the per-server nonce in the path | 204, or 404 for a wrong nonce; triggers only a status re-read for that server (a body `request_id` is adopted if its status shows the server's tag) |
| `GET /admin` | Yan | Cloudflare Access | HTML dashboard |

Errors are `{error}` with a 4xx/5xx status. Examples: `contract/`.

**No credential is ever in a URL.** The ticket travels in a request body; the report secret in a
header; the webhook path carries a per-server nonce that authorizes nothing beyond a status re-read
(Edgegap's `webhook_url` takes no headers). Invocation logs, which record request URLs, are off in
`wrangler.jsonc` regardless.

**Join rules.** Among `starting` and `ready` servers of the version in the selected locations that
are `open` and not full, the lowest client ping wins, then the fullest, then the oldest. Load is the
heartbeat's `players` plus reservations: tickets attached within `RESERVATION_TTL_S` that have not
yet arrived (a heartbeat's player increase marks that many of the oldest as arrived). A location
with no ping ranks after all pinged ones. Otherwise a `starting` row is written and its alarm set,
then Edgegap `/v1/deploy` is called with a city filter for the lowest-ping selected location and a
tag naming the claim. Joins arriving meanwhile attach to that row. If the start fails (refused,
Edgegap error, or no ready within `START_DEADLINE_S`), each waiting ticket is placed once more
(`JOIN_ATTEMPTS` = 2 in all), never in or onto a location that already failed it.

**Unknown deploy outcome.** Only a 4xx answer (not 408) is a refusal. A timeout, lost connection
or 5xx may have created a deployment, so the claim keeps its tickets and no replacement starts; the
alarm looks the deployment up by its tag and adopts it (a ready webhook's `request_id` is adopted
too, once its status shows the tag). Past the start deadline a found deployment is stopped; only a
lookup past the deadline that finds nothing frees the tickets.

**Ready** means Edgegap has reported the address (ready webhook, or the alarm polling status every
`STATUS_POLL_S`) **and** the server has sent a heartbeat.

**Ending.** An ended, lost or failed server moves to `stopping`; the alarm retries the Edgegap stop
every `STATUS_POLL_S` until it succeeds (or Edgegap no longer knows the deployment). Only then is
the row `ended` and its D1 history closed; retention never deletes a row whose stop is unconfirmed.
Every Edgegap call and D1 write is bounded by `PROVIDER_TIMEOUT_S`, and the alarm makes all its
transitions and re-arms itself before running that outbound work, so one slow call holds up no
other row.

**Failure reasons** shown to clients and on `/admin` come from a fixed list (`src/reasons.ts`);
provider error text is never read, stored or logged.

**Locations.** The account's full Edgegap list (every page), cached `LOCATIONS_CACHE_S` per version
object and served stale if Edgegap is down. A location carries a beacon only when one is in the
same city and country; otherwise `beacon` is `null` and the client shows "no estimate". Ordering by
ping is the client's job. Reserved, absent in v1: a per-server ping address on locations and join
candidates, for when servers answer pings themselves.

## Game-server contract (phase B)

The Worker passes these through Edgegap `env_vars` (example: `contract/server-env.json`):

| Variable | Meaning |
|---|---|
| `OVERMATCH_SERVER_ID` | This server's id; goes in the heartbeat and end URLs |
| `OVERMATCH_API` | Worker base URL, e.g. `https://matchmaking.example` |
| `OVERMATCH_NETCODE_KEY` | 64 hex chars = the 32-byte netcode private key (hidden in Edgegap) |
| `OVERMATCH_REPORT_SECRET` | Bearer token for heartbeat and end (hidden in Edgegap) |
| `OVERMATCH_CAPACITY` | Player cap, default 10 |
| `OVERMATCH_HEARTBEAT_INTERVAL_S` | Heartbeat period, default 5 |
| `OVERMATCH_EMPTY_EXIT_S` | Exit after this long with no players, default 300 |

The server must: listen on its fixed UDP port (`game` in the Edgegap version); send its first
heartbeat as soon as it listens and then every interval (`{players, capacity, state}`, full record,
`state` `"open"` or `"closing"`); exit on a 410; and on empty-timeout or any clean exit, POST `end`
and then exit. The Worker drops a ready server after `HEARTBEAT_DEADLINE_S` without a heartbeat and
stops its deployment.

## Configuration

Tunables are `vars` in `wrangler.jsonc` (capacity, heartbeat interval and deadline, start deadline,
status poll, reservation TTL, ticket TTL, empty exit, retention, join attempts, cache TTL, provider
timeout, cost rate, Durable Object location hint). Secrets, set with `wrangler secret put`:

| Secret | What |
|---|---|
| `EDGEGAP_TOKEN` | Edgegap API token (with or without the `token ` prefix) |

`ACCESS_TEAM_DOMAIN` (e.g. `https://<team>.cloudflareaccess.com`) and `ACCESS_AUD` are plain vars,
set in `wrangler.jsonc` once the Access application exists. While either is empty, `/admin` answers
403.

## Local development

```sh
cd backend
npm ci
npm test                 # vitest in the Workers runtime, Edgegap mocked; no network
npm run typecheck        # generates src/worker-configuration.d.ts (gitignored), then tsc
```

`npm run dev` (`wrangler dev`) needs a `.dev.vars` (gitignored) with `EDGEGAP_TOKEN`, plus `npx wrangler d1 migrations apply overmatch-sessions --local`. It
calls the real Edgegap API, so a join there starts a billed deployment.

## Deploy (the Worker by hand, CI only tests it; game versions by CI)

1. `npx wrangler login`.
2. `npx wrangler d1 create overmatch-sessions`; paste the printed `database_id` into
   `wrangler.jsonc`.
3. `npx wrangler d1 migrations apply overmatch-sessions --remote`.
4. `npx wrangler secret put EDGEGAP_TOKEN`.
5. `npx wrangler deploy`.
6. **Access for `/admin`** (Zero Trust dashboard → Access controls → Applications → Add → Self-hosted):
   the application domain is the Worker's hostname with path `admin`; add an Allow policy for
   Yan's email. Copy the application's AUD tag into `ACCESS_AUD` and the team domain into
   `ACCESS_TEAM_DOMAIN` in `wrangler.jsonc`, then deploy again. The Worker verifies the
   `Cf-Access-Jwt-Assertion` header against `<team>/cdn-cgi/access/certs`, so a request that
   bypasses Access is refused anyway.
7. **Edgegap per game version — CI owns it; nothing to do by hand.** Every release tag
   (`.github/workflows/release.yml`, job `register-edgegap`) pushes the server image to
   `ghcr.io/vikng-dev/overmatch-server` (public; tags `<version>` and `<version>-<sha7>`) and
   creates or updates version `<version>` of app `Overmatch` (`EDGEGAP_APP`): the version named
   exactly as the client's `CARGO_PKG_VERSION`, image tag `<version>-<sha7>`, UDP 5888 named `game`
   (`EDGEGAP_PORT_NAME`), `restart_policy: never`, `max_duration` 360 min, 1 vCPU / 1 GB, no env vars
   (the Worker passes the game-server contract above per deployment). The Release is published
   only after that registration. To register a commit without a release, dispatch
   `gh workflow run edgegap-version.yml --ref <branch>`. Both run
   `scripts/ci/edgegap-version.sh`, which holds the request bodies and the reasons for each value;
   it needs the `EDGEGAP_TOKEN` repository secret.

## Built now, deferred behind a seam

| Deferred | Seam |
|---|---|
| Connect tokens instead of a shared per-match key | `TicketResponse.key` becomes a token; the Registry already owns the key |
| Finer sharding than one object per version | `registryFor()` in `src/registry.ts` |
| D1 outbox, Analytics Engine time series | `recordEvent()` in `src/history.ts` |
| WebSocket or write-on-change heartbeats | `heartbeat()`; the body is already the full record |
| Push instead of ticket polling | the ticket already distinguishes `starting` |
| Server-side ping address | reserved optional field on locations and join candidates |
| Another provider | `Provider` in `src/edgegap.ts` |
| CI deploy | `.github/workflows/backend.yml` tests only |
