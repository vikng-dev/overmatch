// `recordEvent` is the single writer of D1 `sessions` (migrations/0001_sessions.sql): one row per
// server lifetime, touched only on transitions. Writes are best effort — a D1 failure is logged and
// never fails the registry operation that caused it. A durable outbox, or a second sink such as
// Analytics Engine, would be added here and nowhere else.

export type Outcome = "ended" | "failed" | "lost";

export type SessionEvent =
	| { kind: "requested"; serverId: string; version: string; location: string; at: number }
	| { kind: "deployed"; serverId: string; deploymentId: string }
	| { kind: "ready"; serverId: string; at: number }
	| { kind: "ended"; serverId: string; at: number; outcome: Outcome; reason: string; peakPlayers: number };

export async function recordEvent(db: D1Database, e: SessionEvent): Promise<void> {
	try {
		switch (e.kind) {
			case "requested":
				await db
					.prepare(
						"INSERT INTO sessions (server_id, version, location, requested_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (server_id) DO NOTHING",
					)
					.bind(e.serverId, e.version, e.location, e.at)
					.run();
				return;
			case "deployed":
				await db.prepare("UPDATE sessions SET deployment_id = ?2 WHERE server_id = ?1").bind(e.serverId, e.deploymentId).run();
				return;
			case "ready":
				await db.prepare("UPDATE sessions SET ready_at = ?2 WHERE server_id = ?1 AND ready_at IS NULL").bind(e.serverId, e.at).run();
				return;
			case "ended":
				await db
					.prepare(
						`UPDATE sessions SET ended_at = ?2, outcome = ?3, reason = ?4,
						   peak_players = MAX(peak_players, ?5),
						   minutes = (?2 - requested_at) / 60000.0
						 WHERE server_id = ?1 AND ended_at IS NULL`,
					)
					.bind(e.serverId, e.at, e.outcome, e.reason, e.peakPlayers)
					.run();
				return;
		}
	} catch (err) {
		console.error("recordEvent failed", e.kind, e.serverId, err);
	}
}
