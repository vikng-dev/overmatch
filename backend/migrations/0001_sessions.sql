-- One row per game-server lifetime, written by src/history.ts on transitions only.
-- Times are milliseconds since the UNIX epoch. `minutes` is ended_at - requested_at: the billed
-- span is DERIVED from it (Edgegap bills from placement, a few seconds later), so the dashboard's
-- cost is an estimate to reconcile against the invoice.
CREATE TABLE sessions (
	server_id     TEXT PRIMARY KEY,
	version       TEXT NOT NULL,
	location      TEXT NOT NULL,
	deployment_id TEXT,
	requested_at  INTEGER NOT NULL,
	ready_at      INTEGER,
	ended_at      INTEGER,
	outcome       TEXT CHECK (outcome IN ('ended', 'failed', 'lost')),
	reason        TEXT,
	peak_players  INTEGER NOT NULL DEFAULT 0,
	minutes       REAL
);

CREATE INDEX sessions_requested_at ON sessions (requested_at);
