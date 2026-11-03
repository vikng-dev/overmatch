Normal ends, losses, startup failures, forgotten claims and attributed orphans mark D1 before
reading logs. Later stops skip capture, including failures. Killed invocations leave
`capture incomplete`; another stop must follow. Concurrent stops can overtake capture.
Mark failure skips reading and still stops, recording what D1 permits.

Resolution: stream at most 16 MiB before parsing; oversize responses leave an error, no log. Drop
`SIM-EVIDENCE`; its detailed tank/track readings are lost, while stats retain pace. Redact and keep
the newest 1 MiB of other lines. Earlier overflow, later lines, failed captures, disappeared
deployments without another stop, and unattributed orphans remain unavailable. Process-exit log
availability is unverified. D1 outages can also lose diagnostics. Retention deletes evidence after
14 days (operator choice). Released servers already support 5-second stats.

The 0.001-second minimum remains unjustified. Page/sweep limits name work budgets.
`scripts/edgegap/test_session.py` still has no runner.

Orchestrator only; not run remotely:

```sh
cd backend
npm ci
npx wrangler d1 migrations apply overmatch-sessions --remote
npx wrangler deploy
```

Read from `backend/` (Wrangler output shape checked locally):

```sh
npx wrangler d1 execute overmatch-sessions --remote --json --command "SELECT log, error FROM server_logs WHERE deployment_id = 'eb3ad1b3be65';" | jq -r '.[] | .results[] | .log // .error // empty'
```
