-- Independent of sessions: even a forgotten claim or a lost history write must leave evidence.
-- captured_at is epoch milliseconds; error contains only diag() output, never provider text.
CREATE TABLE server_logs (
	server_id     TEXT PRIMARY KEY,
	deployment_id TEXT NOT NULL UNIQUE,
	captured_at   INTEGER NOT NULL,
	log           TEXT,
	truncated     INTEGER NOT NULL CHECK (truncated IN (0, 1)),
	error         TEXT
);

CREATE INDEX server_logs_captured_at ON server_logs (captured_at);
