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
| `GET /v1/join/{ticket}` | client | the ticket | `{state: starting\|ready\|failed, address?, port?, key?, reason?}`; poll until not `starting` |
| `POST /v1/servers/{id}/heartbeat` | game server | `Authorization: Bearer <OVERMATCH_REPORT_SECRET>` | 204; **410 = unknown server, exit now**; 401 bad secret |
| `POST /v1/servers/{id}/end` | game server | same | 204 (the Worker stops the deployment); 410 unknown |
| `POST /v1/edgegap/webhook?server=ID&token=T` | Edgegap | `token` (or `X-Webhook-Token` header) = `EDGEGAP_WEBHOOK_SECRET` | 204; the body is ignored, status is re-read |
| `GET /admin` | Yan | Cloudflare Access | HTML dashboard |

Errors are `{error}` with a 4xx/5xx status. Examples: `contract/`.

**Join rules.** Among `starting` and `ready` servers of the version in the selected locations that
are `open` and not full (players + tickets attached within `RESERVATION_TTL_S` < capacity), the
lowest client ping wins, then the fullest, then the oldest. A location with no ping ranks after all
pinged ones. Otherwise a `starting` row is written, then Edgegap `/v1/deploy` is called with a city
filter for the lowest-ping selected location. Joins arriving meanwhile attach to that row. If the
start fails (refused, Edgegap error, or no ready within `START_DEADLINE_S`), each waiting ticket is
placed once more (`JOIN_ATTEMPTS` = 2 in all), never in a location that already failed it.

**Ready** means Edgegap has reported the address (ready webhook, or the alarm polling status every
`STATUS_POLL_S`) **and** the server has sent a heartbeat.

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
status poll, reservation TTL, ticket TTL, empty exit, retention, join attempts, cache TTL, cost
rate, Durable Object location hint). Secrets, set with `wrangler secret put`:

| Secret | What |
|---|---|
| `EDGEGAP_TOKEN` | Edgegap API token (with or without the `token ` prefix) |
| `EDGEGAP_WEBHOOK_SECRET` | Random string; Edgegap webhooks must carry it as `token` |

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

`npm run dev` (`wrangler dev`) needs a `.dev.vars` (gitignored) with `EDGEGAP_TOKEN` and
`EDGEGAP_WEBHOOK_SECRET`, plus `npx wrangler d1 migrations apply overmatch-sessions --local`. It
calls the real Edgegap API, so a join there starts a billed deployment.

## Deploy (by hand; CI only tests)

1. `npx wrangler login`.
2. `npx wrangler d1 create overmatch-sessions`; paste the printed `database_id` into
   `wrangler.jsonc`.
3. `npx wrangler d1 migrations apply overmatch-sessions --remote`.
4. `npx wrangler secret put EDGEGAP_TOKEN`, then `npx wrangler secret put EDGEGAP_WEBHOOK_SECRET`.
5. `npx wrangler deploy`.
6. **Access for `/admin`** (Zero Trust dashboard → Access controls → Applications → Add → Self-hosted):
   the application domain is the Worker's hostname with path `admin`; add an Allow policy for
   Yan's email. Copy the application's AUD tag into `ACCESS_AUD` and the team domain into
   `ACCESS_TEAM_DOMAIN` in `wrangler.jsonc`, then deploy again. The Worker verifies the
   `Cf-Access-Jwt-Assertion` header against `<team>/cdn-cgi/access/certs`, so a request that
   bypasses Access is refused anyway.
7. **Edgegap per game version:** app `Overmatch` (`EDGEGAP_APP`), version named exactly as the
   client's version string, UDP port named `game` (`EDGEGAP_PORT_NAME`), `restart_policy: never`,
   `max_duration` as the backstop. Registering it belongs to the release workflow.

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
