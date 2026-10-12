// One Registry Durable Object per game version holds that version's live servers and join tickets
// in its SQLite storage. Design: .agents/scratch/hosting-2026-10/state-store.md.
//
// Invariants a change here must keep:
// - Find-or-create is check-then-act with no `await` between the read and the `starting` insert.
//   Storage input gates do not cover outbound fetch, so that row — written before the Edgegap call —
//   is the only thing that stops two joins starting two servers. Concurrent joins see it and attach.
// - A server is joinable ("ready") only when Edgegap has reported its address AND it has sent a
//   heartbeat: Edgegap says the container runs, the heartbeat says the game serves.
// - Nothing lives only in memory; every field survives eviction. After any `await`, rows are re-read
//   before they are acted on, and transitions are guarded by `WHERE state = …`.
// - The alarm is idempotent (delivery is at-least-once) and is always set to the earliest deadline;
//   with no rows left it is cleared and the object goes idle. It makes every due transition and
//   persists every row's next attempt time synchronously, re-arms itself, and only then does the
//   outbound work — each call bounded (PROVIDER_TIMEOUT_S) and independent of the others, so one
//   slow call delays no other row.
// - A claim whose deploy outcome is unknown (timeout, lost response, 5xx) is never replaced while
//   its fate is unknown: its tickets stay with it, and the alarm looks the deployment up by the
//   claim's tag (`tagOf`). Found, it is adopted (or stopped, past the deadline); a lookup past the
//   start deadline that finds nothing is the only proof of absence that frees the tickets.
// - A deployment is forgotten only once Edgegap confirms it stopped. Ending a server moves its row to
//   `stopping`; the alarm retries the stop until it succeeds (or Edgegap reports the deployment
//   gone), and only then is the row `ended` and its history closed.
// - A deployment carries its claim's tag (`tagOf`) from the deploy request on, and the claim's row
//   is written here before that request. The orphan sweep (src/sweep.ts) relies on it: a tagged
//   deployment of this version that no live row owns (`liveDeployments`) is stopped.
// - Secrets: the netcode key leaves this object only in a ticket answer for that server; the report
//   secret is stored only as its SHA-256 and never leaves.

import { DurableObject } from "cloudflare:workers";
import { config, type Config, type Env } from "./config";
import { definiteRejection, edgegapFor, ProviderError, type Beacon, type DeploymentStatus, type LocationData, type Provider } from "./edgegap";
import { settleAll } from "./deadline";
import { recordEvent, type Outcome, type SessionEvent } from "./history";
import { timingSafeEqual, newId, randomHex, sha256Hex, versionOf } from "./ids";
import { groupLocations, locationId } from "./locations";
import { diag, REASONS, serverReason } from "./reasons";
import type { HeartbeatRequest, LocationsResponse, TicketResponse } from "./types";

type ServerState = "starting" | "ready" | "stopping" | "ended";

type ServerRow = {
	id: string;
	location: string;
	deployment_id: string | null;
	state: ServerState;
	outcome: Outcome | null;
	reason: string | null;
	players: number;
	capacity: number;
	server_state: "open" | "closing" | null;
	address: string | null;
	port: number | null;
	netcode_key: string;
	secret_hash: string;
	created_at: number;
	ready_at: number | null;
	last_seen: number | null;
	next_poll_at: number | null;
	ended_at: number | null;
	peak_players: number;
	/** calling: the deploy call is in flight; unknown: it ended without an answer; placed: the
	 *  deployment id is known; refused: Edgegap answered no. */
	deploy: "calling" | "unknown" | "placed" | "refused";
	stop_at: number | null;
	webhook_nonce: string;
}

type TicketRow = {
	id: string;
	server_id: string | null;
	locations: string;
	pings: string;
	excluded: string;
	client_ip: string | null;
	origin: string;
	attempts: number;
	failed_reason: string | null;
	created_at: number;
	attached_at: number | null;
	arrived_at: number | null;
}

/** A live server as the admin dashboard sees it: no key, no secret. */
export type ServerSnapshot = {
	id: string;
	location: string;
	deployment_id: string | null;
	state: ServerState;
	outcome: Outcome | null;
	reason: string | null;
	players: number;
	capacity: number;
	server_state: "open" | "closing" | null;
	address: string | null;
	port: number | null;
	created_at: number;
	ready_at: number | null;
	last_seen: number | null;
	ended_at: number | null;
	peak_players: number;
}

export type JoinResult = { ok: true; ticket: string } | { ok: false; status: number; error: string };
export type ServerCallResult = { status: 204 | 400 | 401 | 410; error?: string };

interface Catalog {
	locations: LocationData[];
	beacons: Beacon[];
}

const SCHEMA = `
CREATE TABLE IF NOT EXISTS servers (
	id TEXT PRIMARY KEY,
	location TEXT NOT NULL,
	deployment_id TEXT,
	state TEXT NOT NULL,
	outcome TEXT,
	reason TEXT,
	players INTEGER NOT NULL DEFAULT 0,
	capacity INTEGER NOT NULL,
	server_state TEXT,
	address TEXT,
	port INTEGER,
	netcode_key TEXT NOT NULL,
	secret_hash TEXT NOT NULL,
	created_at INTEGER NOT NULL,
	ready_at INTEGER,
	last_seen INTEGER,
	next_poll_at INTEGER,
	ended_at INTEGER,
	peak_players INTEGER NOT NULL DEFAULT 0,
	deploy TEXT NOT NULL DEFAULT 'calling',
	stop_at INTEGER,
	webhook_nonce TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tickets (
	id TEXT PRIMARY KEY,
	server_id TEXT,
	locations TEXT NOT NULL,
	pings TEXT NOT NULL,
	excluded TEXT NOT NULL DEFAULT '[]',
	client_ip TEXT,
	origin TEXT NOT NULL,
	attempts INTEGER NOT NULL DEFAULT 0,
	failed_reason TEXT,
	created_at INTEGER NOT NULL,
	attached_at INTEGER,
	arrived_at INTEGER
);
CREATE TABLE IF NOT EXISTS catalog (
	k TEXT PRIMARY KEY,
	body TEXT NOT NULL,
	fetched_at INTEGER NOT NULL
);
`;

export class Registry extends DurableObject<Env> {
	private readonly sql: SqlStorage;
	private readonly cfg: Config;
	private readonly provider: Provider;

	constructor(ctx: DurableObjectState, env: Env) {
		super(ctx, env);
		this.sql = ctx.storage.sql;
		this.cfg = config(env);
		this.provider = edgegapFor(env, this.cfg.providerTimeoutMs);
		this.sql.exec(SCHEMA);
	}

	// ---- client API ---------------------------------------------------------------------------

	async locations(version: string): Promise<{ ok: true; body: LocationsResponse } | { ok: false; error: string }> {
		let cat: Catalog;
		try {
			cat = await this.catalog();
		} catch (err) {
			return { ok: false, error: `location list unavailable: ${String(err)}` };
		}
		const matches = new Map<string, number>();
		for (const r of this.sql
			.exec<{ location: string; n: number }>("SELECT location, COUNT(*) AS n FROM servers WHERE state IN ('starting', 'ready') GROUP BY location")
			.toArray()) {
			matches.set(r.location, r.n);
		}
		return { ok: true, body: { version, continents: groupLocations(cat.locations, cat.beacons, matches) } };
	}

	async join(
		version: string,
		locations: string[],
		pings: Record<string, number>,
		clientIp: string | null,
		origin: string,
	): Promise<JoinResult> {
		let known: Set<string>;
		try {
			known = new Set((await this.catalog()).locations.map(locationId));
		} catch (err) {
			return { ok: false, status: 503, error: `location list unavailable: ${String(err)}` };
		}
		const unknown = locations.filter((l) => !known.has(l));
		if (unknown.length) return { ok: false, status: 400, error: `unknown locations: ${unknown.join(", ")}` };

		const ticket = newId(version);
		this.sql.exec(
			"INSERT INTO tickets (id, locations, pings, client_ip, origin, created_at) VALUES (?, ?, ?, ?, ?, ?)",
			ticket,
			JSON.stringify(locations),
			JSON.stringify(pings),
			clientIp,
			origin,
			Date.now(),
		);
		await this.place(version, ticket);
		await this.ensureAlarm(Date.now() + this.cfg.ticketTtlMs);
		return { ok: true, ticket };
	}

	ticket(ticketId: string): TicketResponse | null {
		const t = this.sql.exec<TicketRow>("SELECT * FROM tickets WHERE id = ?", ticketId).toArray()[0];
		if (!t || t.created_at + this.cfg.ticketTtlMs <= Date.now()) return null;
		if (t.failed_reason) return { state: "failed", reason: t.failed_reason };
		const s = t.server_id ? this.server(t.server_id) : null;
		if (!s) return { state: "starting" };
		if (s.state === "ready") return { state: "ready", address: s.address!, port: s.port!, key: s.netcode_key };
		if (s.state === "stopping" && !this.fateKnown(s)) return { state: "starting" };
		if (s.state === "stopping" || s.state === "ended") return { state: "failed", reason: publicReason(s.reason) };
		return { state: "starting" };
	}

	// ---- server API ---------------------------------------------------------------------------

	async heartbeat(serverId: string, secret: string, hb: HeartbeatRequest): Promise<ServerCallResult> {
		const s = this.server(serverId);
		if (!s || !live(s)) return { status: 410 };
		if (!(await this.authorized(s, secret))) return { status: 401, error: "bad report secret" };
		const now = Date.now();
		const written = this.sql.exec(
			`UPDATE servers SET players = ?, capacity = ?, server_state = ?, last_seen = ?, peak_players = MAX(peak_players, ?)
			 WHERE id = ? AND state IN ('starting', 'ready')`,
			hb.players,
			hb.capacity,
			hb.state,
			now,
			hb.players,
			serverId,
		).rowsWritten;
		if (!written) return { status: 410 };
		this.acknowledgeArrivals(serverId, hb.players - s.players, now);
		if (s.state === "starting") {
			if (!(await this.promote(serverId)) && s.deployment_id) await this.refreshStatus(serverId);
		}
		await this.ensureAlarm(now + this.cfg.heartbeatDeadlineMs);
		return { status: 204 };
	}

	async end(serverId: string, secret: string, reason: string): Promise<ServerCallResult> {
		const s = this.server(serverId);
		if (!s || !live(s)) return { status: 410 };
		if (!(await this.authorized(s, secret))) return { status: 401, error: "bad report secret" };
		await this.finish(serverId, "ended", serverReason(reason));
		return { status: 204 };
	}

	/** An Edgegap webhook is a hint only (unsigned, never retried): re-read the status and act on
	 *  that. Its body's request id is adopted for a claim whose deploy response was lost, but only
	 *  once the status read shows the claim's tag. */
	async webhook(serverId: string, nonce: string, requestId: string | null): Promise<boolean> {
		const s = this.server(serverId);
		if (!s || !timingSafeEqual(nonce, s.webhook_nonce)) return false;
		if (s.state === "ended") return true;
		if (!s.deployment_id) {
			if (!requestId) return true;
			let st: DeploymentStatus;
			try {
				st = await this.provider.status(requestId);
			} catch (err) {
				console.warn("webhook status read failed", serverId, diag(err));
				return true;
			}
			if (st.tags.includes(tagOf(serverId))) await this.adopt(serverId, requestId);
			return true;
		}
		if (live(s)) await this.refreshStatus(serverId);
		return true;
	}

	// ---- dashboard ----------------------------------------------------------------------------

	snapshot(): ServerSnapshot[] {
		return this.sql
			.exec<ServerSnapshot>(
				`SELECT id, location, deployment_id, state, outcome, reason, players, capacity, server_state, address, port,
				        created_at, ready_at, last_seen, ended_at, peak_players
				 FROM servers ORDER BY created_at`,
			)
			.toArray();
	}

	// ---- orphan sweep -------------------------------------------------------------------------

	/** Of the listed deployments, the ones this registry still runs: the claim's row is `starting` or
	 *  `ready` and its deployment id is the listed one or not yet known (deploy in flight or of
	 *  unknown outcome, which `reconcile` settles by tag). Every other one is the sweep's to stop. */
	liveDeployments(claims: { serverId: string; deploymentId: string }[]): string[] {
		return claims
			.filter(({ serverId, deploymentId }) => {
				const s = this.server(serverId);
				return !!s && live(s) && (s.deployment_id === null || s.deployment_id === deploymentId);
			})
			.map((c) => c.deploymentId);
	}

	// ---- alarm --------------------------------------------------------------------------------

	async alarm(): Promise<void> {
		const now = Date.now();
		const retry: string[] = [];
		const work: (() => Promise<unknown>)[] = [];

		for (const r of this.sql.exec<ServerRow>("SELECT * FROM servers WHERE state = 'starting'").toArray()) {
			if (r.created_at + this.cfg.startDeadlineMs <= now) {
				retry.push(...(this.beginStop(r.id, "failed", REASONS.startTimeout, "starting", now) ?? []));
			} else if ((r.next_poll_at ?? 0) <= now) {
				this.sql.exec("UPDATE servers SET next_poll_at = ? WHERE id = ?", now + this.cfg.statusPollMs, r.id);
				if (r.deployment_id) work.push(() => this.refreshStatus(r.id));
				else if (this.fateUnknown(r, now)) work.push(() => this.reconcile(r.id));
			}
		}
		const silent = this.sql
			.exec<{ id: string }>("SELECT id FROM servers WHERE state = 'ready' AND last_seen <= ?", now - this.cfg.heartbeatDeadlineMs)
			.toArray();
		for (const { id } of silent) retry.push(...(this.beginStop(id, "lost", REASONS.serverLost, "ready", now) ?? []));
		const stopping = this.sql
			.exec<{ id: string }>("SELECT id FROM servers WHERE state = 'stopping' AND COALESCE(stop_at, 0) <= ?", now)
			.toArray();
		for (const { id } of stopping) {
			this.sql.exec("UPDATE servers SET stop_at = ? WHERE id = ?", now + this.cfg.statusPollMs, id);
			work.push(() => this.tryStop(id));
		}

		this.sql.exec("DELETE FROM tickets WHERE created_at <= ?", now - this.cfg.ticketTtlMs);
		this.sql.exec("DELETE FROM servers WHERE state = 'ended' AND ended_at <= ?", now - this.cfg.endedRetentionMs);
		await this.reschedule();

		await settleAll(work, OUTBOUND_CONCURRENCY);
		await this.replace(retry);
		await this.reschedule();
	}

	// ---- internals ----------------------------------------------------------------------------

	private server(id: string): ServerRow | undefined {
		return this.sql.exec<ServerRow>("SELECT * FROM servers WHERE id = ?", id).toArray()[0];
	}

	private async authorized(s: ServerRow, secret: string): Promise<boolean> {
		return secret.length > 0 && timingSafeEqual(await sha256Hex(secret), s.secret_hash);
	}

	private async catalog(): Promise<Catalog> {
		const row = this.sql.exec<{ body: string; fetched_at: number }>("SELECT body, fetched_at FROM catalog WHERE k = 'edgegap'").toArray()[0];
		if (row && row.fetched_at + this.cfg.locationsCacheMs > Date.now()) return JSON.parse(row.body) as Catalog;
		try {
			const [locations, beacons] = await Promise.all([this.provider.locations(), this.provider.beacons()]);
			const cat: Catalog = { locations, beacons };
			this.sql.exec(
				"INSERT INTO catalog (k, body, fetched_at) VALUES ('edgegap', ?, ?) ON CONFLICT (k) DO UPDATE SET body = excluded.body, fetched_at = excluded.fetched_at",
				JSON.stringify(cat),
				Date.now(),
			);
			return cat;
		} catch (err) {
			if (row) {
				console.warn("location refresh failed, serving the stale list", diag(err));
				return JSON.parse(row.body) as Catalog;
			}
			throw err;
		}
	}

	/** Reservations: a ticket attached within the TTL and not yet seen to arrive counts against
	 *  capacity, because a player's arrival reaches `players` only a heartbeat later. */
	private load(s: ServerRow, now: number): number {
		const r = this.sql
			.exec<{ n: number }>(
				"SELECT COUNT(*) AS n FROM tickets WHERE server_id = ? AND attached_at > ? AND arrived_at IS NULL",
				s.id,
				now - this.cfg.reservationTtlMs,
			)
			.one().n;
		return s.players + r;
	}

	/** A heartbeat's player increase is that many arrivals: the oldest outstanding reservations are
	 *  marked arrived and stop counting. Arrivals are not attributable to tickets, so a departure and
	 *  an arrival inside one interval leave a reservation outstanding until its TTL — the safe side. */
	private acknowledgeArrivals(serverId: string, increase: number, now: number): void {
		if (increase <= 0) return;
		this.sql.exec(
			`UPDATE tickets SET arrived_at = ? WHERE id IN (
				SELECT id FROM tickets WHERE server_id = ? AND arrived_at IS NULL AND attached_at > ? ORDER BY attached_at LIMIT ?
			)`,
			now,
			serverId,
			now - this.cfg.reservationTtlMs,
			increase,
		);
	}

	/** Attach the ticket to the best open server in its locations, or start one. The read and the
	 *  `starting` insert run with no `await` between them; see the module invariants. */
	private async place(version: string, ticketId: string): Promise<void> {
		// Minted before the read below: from that read to the `starting` insert nothing may await.
		const reportSecret = randomHex(32);
		const secretHash = await sha256Hex(reportSecret);

		const t = this.sql.exec<TicketRow>("SELECT * FROM tickets WHERE id = ?", ticketId).toArray()[0];
		if (!t) return;
		const now = Date.now();
		const wanted: string[] = JSON.parse(t.locations);
		const pings: Record<string, number> = JSON.parse(t.pings);
		const excluded = new Set<string>(JSON.parse(t.excluded));
		const ping = (loc: string) => (Number.isFinite(pings[loc]) ? pings[loc] : Infinity);

		const candidates = this.sql
			.exec<ServerRow>("SELECT * FROM servers WHERE state IN ('starting', 'ready') AND (server_state IS NULL OR server_state = 'open')")
			.toArray()
			.filter((s) => wanted.includes(s.location) && !excluded.has(s.location))
			.map((s) => ({ s, load: this.load(s, now) }))
			.filter(({ s, load }) => load < s.capacity)
			.sort((a, b) => ping(a.s.location) - ping(b.s.location) || b.load - a.load || a.s.created_at - b.s.created_at);
		if (candidates.length) {
			this.sql.exec("UPDATE tickets SET server_id = ?, attached_at = ? WHERE id = ?", candidates[0].s.id, now, ticketId);
			return;
		}

		const where = wanted
			.map((loc, i) => ({ loc, i }))
			.filter(({ loc }) => !excluded.has(loc))
			.sort((a, b) => ping(a.loc) - ping(b.loc) || a.i - b.i)[0]?.loc;
		if (!where) {
			this.sql.exec("UPDATE tickets SET failed_reason = ? WHERE id = ?", REASONS.noLocation, ticketId);
			return;
		}

		const serverId = newId(version);
		const netcodeKey = randomHex(32);
		const webhookNonce = randomHex(16);
		this.sql.exec(
			`INSERT INTO servers (id, location, state, capacity, netcode_key, secret_hash, created_at, next_poll_at, deploy, webhook_nonce)
			 VALUES (?, ?, 'starting', ?, ?, ?, ?, ?, 'calling', ?)`,
			serverId,
			where,
			this.cfg.capacity,
			netcodeKey,
			secretHash,
			now,
			now + this.cfg.statusPollMs,
			webhookNonce,
		);
		this.sql.exec("UPDATE tickets SET server_id = ?, attached_at = ? WHERE id = ?", serverId, now, ticketId);
		// The claim's deadline is durable before any outbound await (D1, Edgegap): a stalled call or an
		// eviction from here on still leaves an alarm that reaps the row.
		await this.ensureAlarm(Math.min(now + this.cfg.statusPollMs, now + this.cfg.startDeadlineMs));

		await this.record({ kind: "requested", serverId, version, location: where, at: now });
		let deploymentId: string;
		try {
			deploymentId = await this.provider.deploy({
				version,
				city: where,
				tag: tagOf(serverId),
				clientIp: t.client_ip,
				webhookUrl: `${t.origin}/v1/edgegap/webhook/${encodeURIComponent(serverId)}/${webhookNonce}`,
				env: [
					{ key: "OVERMATCH_SERVER_ID", value: serverId, hidden: false },
					{ key: "OVERMATCH_API", value: t.origin, hidden: false },
					{ key: "OVERMATCH_NETCODE_KEY", value: netcodeKey, hidden: true },
					{ key: "OVERMATCH_REPORT_SECRET", value: reportSecret, hidden: true },
					{ key: "OVERMATCH_CAPACITY", value: String(this.cfg.capacity), hidden: false },
					{ key: "OVERMATCH_HEARTBEAT_INTERVAL_S", value: String(this.cfg.heartbeatIntervalMs / 1000), hidden: false },
					{ key: "OVERMATCH_EMPTY_EXIT_S", value: String(this.cfg.emptyExitS), hidden: false },
				],
			});
		} catch (err) {
			console.warn("deploy failed", serverId, diag(err));
			if (!definiteRejection(err)) {
				// A deployment may exist: keep the claim and its tickets; the alarm reconciles by tag.
				this.sql.exec("UPDATE servers SET deploy = 'unknown' WHERE id = ? AND deploy = 'calling'", serverId);
				if (this.server(serverId)?.state === "stopping") await this.tryStop(serverId);
				return;
			}
			this.sql.exec("UPDATE servers SET deploy = 'refused' WHERE id = ?", serverId);
			await this.finish(serverId, "failed", deployFailure(err), "starting");
			await this.tryStop(serverId); // ended while the call was in flight: nothing to stop now
			return;
		}
		await this.adopt(serverId, deploymentId, false);
	}

	/** Record a claim's deployment id, whatever the row's state: a claim that ended while its id was
	 *  unknown is `stopping`, and its stop needs the id. An id recovered late (`poll`) is followed by
	 *  a status read, since the ready webhook may already have come and gone. */
	private async adopt(serverId: string, deploymentId: string, poll = true): Promise<void> {
		const row = this.server(serverId);
		if (!row) {
			// The claim was forgotten (retention) before its deploy answered: one stop is all that is left.
			await this.provider.stop(deploymentId).catch((err) => console.error("stop of forgotten claim failed", serverId, diag(err)));
			return;
		}
		if (row.state === "ended" && !row.deployment_id) {
			// Absence was accepted, then the deploy answered after all: reopen the row to stop it.
			this.sql.exec(
				"UPDATE servers SET state = 'stopping', stop_at = ?, deployment_id = ?, deploy = 'placed' WHERE id = ? AND state = 'ended'",
				Date.now(),
				deploymentId,
				serverId,
			);
			await this.tryStop(serverId);
			await this.reschedule();
			return;
		}
		const written = this.sql.exec(
			"UPDATE servers SET deployment_id = ?, deploy = 'placed' WHERE id = ? AND deployment_id IS NULL",
			deploymentId,
			serverId,
		).rowsWritten;
		if (!written) return;
		await this.record({ kind: "deployed", serverId, version: versionOf(serverId)!, location: row.location, requestedAt: row.created_at, deploymentId });
		const s = this.server(serverId);
		if (s?.state === "stopping") {
			// The id makes the claim's fate known: its deferred tickets may now be placed elsewhere.
			const retry = this.releaseTickets(s, s.reason ?? REASONS.startFailed);
			await this.tryStop(serverId);
			await this.replace(retry);
		} else if (s?.state === "starting" && poll) {
			await this.refreshStatus(serverId);
		}
	}

	/** Look a claim of unknown fate up by its tag. Found: adopted. Not found: the claim waits, unless
	 *  it is `stopping` past its start deadline, when the absence is accepted and the row ends. */
	private async reconcile(serverId: string): Promise<void> {
		const s = this.server(serverId);
		if (!s || s.deployment_id || s.state === "ended") return;
		let found: string | null;
		try {
			found = await this.provider.findByTag(tagOf(serverId));
		} catch (err) {
			console.warn("deployment lookup failed", serverId, diag(err));
			return;
		}
		if (found) return this.adopt(serverId, found);
		const now = Date.now();
		const fresh = this.server(serverId);
		if (fresh?.state === "stopping" && !fresh.deployment_id && fresh.created_at + this.cfg.startDeadlineMs <= now) {
			this.sql.exec("UPDATE servers SET deploy = 'refused' WHERE id = ? AND deployment_id IS NULL", serverId);
			await this.markEnded(serverId);
		}
	}

	/** Whether the claim's deploy outcome is settled: an id, or a refusal. */
	private fateKnown(s: ServerRow): boolean {
		return s.deployment_id !== null || s.deploy === "refused";
	}

	/** No id and no refusal, and no live call that could still answer. The call is bounded by
	 *  PROVIDER_TIMEOUT_S after a bounded D1 write, so a `calling` row older than three bounds
	 *  belongs to an object that was evicted mid-call. */
	private fateUnknown(s: ServerRow, now: number): boolean {
		if (this.fateKnown(s)) return false;
		return s.deploy === "unknown" || s.created_at + 3 * this.cfg.providerTimeoutMs <= now;
	}

	private async refreshStatus(serverId: string): Promise<void> {
		const before = this.server(serverId);
		if (!before?.deployment_id || !live(before)) return;
		let st: DeploymentStatus;
		try {
			st = await this.provider.status(before.deployment_id);
		} catch (err) {
			console.warn("status poll failed", serverId, diag(err));
			return;
		}
		const s = this.server(serverId);
		if (!s || !live(s)) return;
		switch (st.phase) {
			case "ready":
				this.sql.exec("UPDATE servers SET address = ?, port = ? WHERE id = ? AND state != 'ended'", st.address, st.port, serverId);
				await this.promote(serverId);
				return;
			case "error":
				await this.finish(serverId, "failed", REASONS.startFailed);
				return;
			case "terminated":
			case "gone":
				await this.finish(serverId, s.state === "starting" ? "failed" : "ended", s.state === "starting" ? REASONS.startFailed : REASONS.providerTerminated);
				return;
			case "pending":
				return;
		}
	}

	private async promote(serverId: string): Promise<boolean> {
		const now = Date.now();
		const n = this.sql.exec(
			`UPDATE servers SET state = 'ready', ready_at = ?
			 WHERE id = ? AND state = 'starting' AND address IS NOT NULL AND last_seen IS NOT NULL`,
			now,
			serverId,
		).rowsWritten;
		if (n) await this.record({ kind: "ready", serverId, at: now });
		return n > 0;
	}

	/** End a live server: move it to `stopping`, give the tickets of a server that never became ready
	 *  another placement (bounded by JOIN_ATTEMPTS), and try the stop. The row stays until the stop
	 *  is confirmed (`tryStop`). */
	private async finish(serverId: string, outcome: Outcome, reason: string, from?: "starting" | "ready"): Promise<void> {
		const retry = this.beginStop(serverId, outcome, reason, from);
		if (retry === null) return;
		await this.tryStop(serverId);
		await this.replace(retry);
		await this.reschedule();
	}

	/** The synchronous half of `finish`: the transition and the ticket bookkeeping. Returns the
	 *  tickets to place again, or null when the row was not live (or not in `from`). */
	private beginStop(serverId: string, outcome: Outcome, reason: string, from?: "starting" | "ready", now = Date.now()): string[] | null {
		const s = this.server(serverId);
		if (!s || !live(s) || (from && s.state !== from)) return null;
		const moved = this.sql.exec(
			"UPDATE servers SET state = 'stopping', outcome = ?, reason = ?, stop_at = ? WHERE id = ? AND state = ?",
			outcome,
			reason,
			now,
			serverId,
			s.state,
		).rowsWritten;
		if (!moved) return null;
		// Tickets of a claim whose deployment may exist stay with it until its fate is known (`adopt`,
		// `reconcile`): placing them now could start a second server for the same players.
		return s.state === "starting" && this.fateKnown(s) ? this.releaseTickets(s, reason) : [];
	}

	/** Detach the waiting tickets of a server that never became ready: each gets another placement
	 *  (bounded by JOIN_ATTEMPTS, away from this location) or fails with `reason`. Returns the ids to
	 *  place again. */
	private releaseTickets(s: ServerRow, reason: string): string[] {
		if (s.ready_at !== null) return [];
		const orphans = this.sql.exec<TicketRow>("SELECT * FROM tickets WHERE server_id = ? AND failed_reason IS NULL", s.id).toArray();
		const retry: string[] = [];
		for (const t of orphans) {
			const excluded = new Set<string>(JSON.parse(t.excluded));
			excluded.add(s.location);
			const attempts = t.attempts + 1;
			const left = (JSON.parse(t.locations) as string[]).some((l) => !excluded.has(l));
			if (attempts >= this.cfg.joinAttempts || !left) {
				this.sql.exec("UPDATE tickets SET attempts = ?, failed_reason = ? WHERE id = ?", attempts, reason, t.id);
			} else {
				this.sql.exec(
					"UPDATE tickets SET attempts = ?, excluded = ?, server_id = NULL, attached_at = NULL WHERE id = ?",
					attempts,
					JSON.stringify([...excluded]),
					t.id,
				);
				retry.push(t.id);
			}
		}
		return retry;
	}

	private async replace(ticketIds: string[]): Promise<void> {
		for (const id of ticketIds) {
			const version = versionOf(id);
			if (version) await this.place(version, id);
		}
	}

	private record(e: SessionEvent): Promise<void> {
		return recordEvent(this.env.DB, e, this.cfg.providerTimeoutMs);
	}

	/** One stop attempt for a `stopping` row. Success (or Edgegap no longer knowing the deployment)
	 *  ends the row and closes its history; failure leaves it `stopping` with the next attempt time.
	 *  A row whose deploy call is still in flight waits for the id. */
	private async tryStop(serverId: string): Promise<void> {
		const s = this.server(serverId);
		if (s?.state !== "stopping") return;
		const now = Date.now();
		if (!s.deployment_id) {
			if (s.deploy === "refused") return this.markEnded(serverId);
			this.sql.exec("UPDATE servers SET stop_at = ? WHERE id = ?", now + this.cfg.statusPollMs, serverId);
			if (this.fateUnknown(s, now)) await this.reconcile(serverId);
			return;
		}
		this.sql.exec("UPDATE servers SET stop_at = ? WHERE id = ?", now + this.cfg.statusPollMs, serverId);
		try {
			await this.provider.stop(s.deployment_id);
		} catch (err) {
			console.error("stop failed, will retry", serverId, diag(err));
			return;
		}
		await this.markEnded(serverId);
	}

	private async markEnded(serverId: string): Promise<void> {
		const s = this.server(serverId);
		if (s?.state !== "stopping") return;
		const now = Date.now();
		if (!this.sql.exec("UPDATE servers SET state = 'ended', ended_at = ? WHERE id = ? AND state = 'stopping'", now, serverId).rowsWritten) return;
		const retry = this.releaseTickets(s, s.reason ?? REASONS.startFailed);
		await this.record({
			kind: "ended",
			serverId,
			at: now,
			outcome: s.outcome ?? "ended",
			reason: s.reason ?? REASONS.serverEnded,
			peakPlayers: s.peak_players,
		});
		await this.replace(retry);
	}

	private async ensureAlarm(at: number): Promise<void> {
		const current = await this.ctx.storage.getAlarm();
		if (current === null || at < current) await this.ctx.storage.setAlarm(at);
	}

	private async reschedule(): Promise<void> {
		const c = this.cfg;
		const r = this.sql
			.exec<{ t: number | null }>(
				`SELECT MIN(t) AS t FROM (
					SELECT MIN(created_at + ?, COALESCE(next_poll_at, 0)) AS t FROM servers WHERE state = 'starting'
					UNION ALL SELECT last_seen + ? FROM servers WHERE state = 'ready'
					UNION ALL SELECT COALESCE(stop_at, 0) FROM servers WHERE state = 'stopping'
					UNION ALL SELECT ended_at + ? FROM servers WHERE state = 'ended'
					UNION ALL SELECT created_at + ? FROM tickets
				)`,
				c.startDeadlineMs,
				c.heartbeatDeadlineMs,
				c.endedRetentionMs,
				c.ticketTtlMs,
			)
			.one().t;
		if (r === null) await this.ctx.storage.deleteAlarm();
		else await this.ctx.storage.setAlarm(r);
	}
}

/** The sharding seam: one Registry per game version. Finer sharding is a change here alone. */
export function registryFor(env: Env, version: string): DurableObjectStub<Registry> {
	const hint = env.LOCATION_HINT as DurableObjectLocationHint | "";
	return env.REGISTRY.getByName(version, hint ? { locationHint: hint } : undefined);
}

/** Ticket-facing reason for an ended server: an allowlisted text, or the generic one. A server's own
 *  end reason (`server: …`) is history only. */
function publicReason(stored: string | null): string {
	return (Object.values(REASONS) as string[]).includes(stored ?? "") ? stored! : REASONS.serverEnded;
}

/** A refused deploy (4xx) is the provider's "no"; 422 is its no-capacity answer (MEASURED in the
 *  risk test). Anything else is the provider not answering. */
function deployFailure(err: unknown): string {
	if (err instanceof ProviderError && err.status === 422) return REASONS.noCapacity;
	if (err instanceof ProviderError && err.status >= 400 && err.status < 500) return REASONS.startRefused;
	return REASONS.providerUnavailable;
}

/** Below the platform's six simultaneous outbound connections per invocation. */
const OUTBOUND_CONCURRENCY = 5;

function live(s: ServerRow): boolean {
	return s.state === "starting" || s.state === "ready";
}

/** The Edgegap tag that identifies a claim's deployment: the random half of the server id (32 hex,
 *  35 characters with the prefix; Edgegap allows 40). */
export function tagOf(serverId: string): string {
	return `om-${serverId.slice(serverId.indexOf(".") + 1)}`;
}
