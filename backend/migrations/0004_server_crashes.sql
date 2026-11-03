-- Independent of log capture: a panic can arrive before the deployment id is known.
CREATE TABLE server_crashes (
    server_id TEXT PRIMARY KEY,
    captured_at INTEGER NOT NULL,
    crash TEXT NOT NULL,
    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1))
);
CREATE INDEX server_crashes_captured_at ON server_crashes (captured_at);
