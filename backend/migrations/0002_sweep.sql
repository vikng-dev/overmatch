-- The orphan sweep's last result (src/sweep.ts): one row, overwritten by every sweep, shown on
-- /admin. `error` is a diag() string, never provider text.
CREATE TABLE sweep (
	id            INTEGER PRIMARY KEY CHECK (id = 1),
	at            INTEGER NOT NULL,
	listed        INTEGER NOT NULL,
	unattributed  INTEGER NOT NULL,
	stopped       INTEGER NOT NULL,
	failed        INTEGER NOT NULL,
	error         TEXT
);
