-- The orphan sweep's state (src/sweep.ts): one row, overwritten by every sweep. `suspects` is a JSON
-- object of deployment id to the time (ms since the UNIX epoch) the sweep first saw it with an `om-`
-- tag but no routable version. `error` is a diag() string, never provider text.
CREATE TABLE sweep (
	id        INTEGER PRIMARY KEY CHECK (id = 1),
	at        INTEGER NOT NULL,
	listed    INTEGER NOT NULL,
	stopped   INTEGER NOT NULL,
	failed    INTEGER NOT NULL,
	error     TEXT,
	suspects  TEXT NOT NULL DEFAULT '{}'
);
