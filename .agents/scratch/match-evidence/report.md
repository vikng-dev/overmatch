New match deployments enable stats every 5 seconds (Resolution: sampling detail versus log size).
The released `v0.7.0-alpha.10` source already reads this lever; no game release is required.

Normal end, heartbeat loss, startup failure, late forgotten claims and attributed orphan stops all
archive before stopping. D1 keeps the redacted UTF-8 tail, deployment/server ids, capture time,
truncation flag, or a safe failure diagnostic. Successful snapshots survive stop retries; cron
expires snapshots and failures after 14 days (operator retention choice). The 1 MiB bound is
DERIVED from the supplied log-size estimate plus headroom.

Capture failures never veto stops. D1 outages can lose evidence and its diagnostic. Later lines,
unattributed orphans, and deployments disappearing without a following Worker stop remain lost.
`cmd_logs` says deployment stops delete provider logs; availability after process exit alone is
unverified. A 404 is recorded when capture is attempted.

Orchestrator commands after merge, from the repository root (not run here):

```sh
cd backend
npm ci
npx wrangler d1 migrations apply overmatch-sessions --remote
npx wrangler deploy
```

Read by deployment id from `backend/`:

```sh
npx wrangler d1 execute overmatch-sessions --remote --command "SELECT server_id, captured_at, truncated, error, log FROM server_logs WHERE deployment_id = 'eb3ad1b3be65';"
```
