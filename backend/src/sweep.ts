// The orphan sweep. Every minute (`triggers.crons` in wrangler.jsonc, `scheduled` in index.ts) the
// Worker lists every Edgegap deployment and stops each one tagged `om-<hex>` (`tagOf` in
// registry.ts) that no live claim owns. A failed stop needs no bookkeeping: the deployment is still
// listed a minute later and judged again, until Edgegap no longer lists it.
//
// Ownership. The tag carries the server id's random half but not its version, and the Registry
// object is per version, so D1 `sessions` is the index from tag to server id and version; the
// version's Registry is the authority on liveness (`liveDeployments`).
// - History row ended: stop. A row ends only once its stop was confirmed, so a listed deployment
//   behind it leaked.
// - History row open: stop unless the Registry still runs it (`starting` or `ready`, with this
//   deployment id or none yet — a deploy in flight or of unknown outcome is kept).
// - No history row: a suspect, stopped once the sweep has seen it unmatched for START_DEADLINE_S.
//   The row is written before the deploy call and upserted again when the deploy id becomes known
//   (`deployed` in history.ts), which the Registry settles by tag within the start deadline; so a
//   claim still unmatched past it lost both writes, and anything else tagged `om-` is not ours to
//   keep. The grace is what keeps a write still in flight from costing a live server.
// - D1 or a Registry unreachable: nothing it would have judged is stopped this sweep.
//
// Bounds per sweep: SWEEP_MAX_PAGES listing pages and SWEEP_MAX_STOPS stops (the rest wait a
// minute), CONCURRENCY calls in flight, each bounded by PROVIDER_TIMEOUT_S.

import { config, type Env } from "./config";
import { bounded, settleAll } from "./deadline";
import { edgegapFor } from "./edgegap";
import { diag } from "./reasons";
import { registryFor } from "./registry";

export const SWEEP_MAX_PAGES = 10;
export const SWEEP_MAX_STOPS = 25;
/** Below the platform's six simultaneous outbound connections per invocation. */
const CONCURRENCY = 5;
/** Bound parameters per D1 statement; D1 allows 100. */
const D1_BATCH = 90;
const TAG = /^om-([0-9a-f]{32})$/;

export interface SweepResult {
	at: number;
	/** Deployments carrying an `om-` tag. */
	listed: number;
	stopped: number;
	failed: number;
	/** The last failure, as diag() text. */
	error: string | null;
}

interface HistoryRow {
	server_id: string;
	version: string;
	ended_at: number | null;
}

export async function sweep(env: Env, now = Date.now()): Promise<SweepResult> {
	const cfg = config(env);
	const timeout = cfg.providerTimeoutMs;
	const provider = edgegapFor(env, timeout);
	const result: SweepResult = { at: now, listed: 0, stopped: 0, failed: 0, error: null };
	const fail = (what: string, err: unknown) => {
		result.error = `${what}: ${diag(err)}`;
		console.error("sweep", result.error);
	};

	let previous: Record<string, number> = {};
	try {
		const row = await bounded(timeout, () => env.DB.prepare("SELECT suspects FROM sweep WHERE id = 1").first<{ suspects: string }>());
		if (row) previous = JSON.parse(row.suspects) as Record<string, number>;
	} catch (err) {
		fail("state read", err);
	}

	// Tag hex → listed deployment ids carrying it.
	const tagged = new Map<string, string[]>();
	try {
		const out = await provider.deployments(SWEEP_MAX_PAGES);
		if (out.truncated) console.warn("sweep: listing truncated at", SWEEP_MAX_PAGES, "pages");
		for (const d of out.items) {
			for (const t of d.tags) {
				const hex = TAG.exec(t)?.[1];
				if (hex) tagged.set(hex, [...(tagged.get(hex) ?? []), d.id]);
			}
		}
	} catch (err) {
		fail("list", err);
		await save(env, result, previous, timeout);
		return result;
	}
	result.listed = [...tagged.values()].reduce((n, ids) => n + ids.length, 0);

	let history: Map<string, HistoryRow>;
	try {
		history = await lookup(env, [...tagged.keys()], timeout);
	} catch (err) {
		fail("history read", err);
		await save(env, result, previous, timeout);
		return result;
	}

	const stop: { id: string; why: string }[] = [];
	const suspects: Record<string, number> = {};
	const open = new Map<string, { serverId: string; deploymentId: string }[]>();
	for (const [hex, ids] of tagged) {
		const row = history.get(hex);
		for (const id of ids) {
			if (!row) {
				// Kept while its stop is unconfirmed, so a failed stop is retried without a fresh grace.
				suspects[id] = previous[id] ?? now;
				if (suspects[id] + cfg.startDeadlineMs <= now) stop.push({ id, why: "unknown" });
			} else if (row.ended_at !== null) {
				stop.push({ id, why: "ended" });
			} else {
				open.set(row.version, [...(open.get(row.version) ?? []), { serverId: row.server_id, deploymentId: id }]);
			}
		}
	}

	await settleAll(
		[...open].map(([version, claims]) => async () => {
			try {
				const keep = new Set<string>(await bounded(timeout, async () => registryFor(env, version).liveDeployments(claims)));
				for (const c of claims) if (!keep.has(c.deploymentId)) stop.push({ id: c.deploymentId, why: "not live" });
			} catch (err) {
				fail("registry", err);
			}
		}),
		CONCURRENCY,
	);

	if (stop.length > SWEEP_MAX_STOPS) console.warn("sweep:", stop.length - SWEEP_MAX_STOPS, "stops wait for the next sweep");
	await settleAll(
		stop.slice(0, SWEEP_MAX_STOPS).map(({ id, why }) => async () => {
			try {
				await provider.stop(id);
				result.stopped++;
				console.warn("sweep stopped", id, why);
			} catch (err) {
				result.failed++;
				fail("stop", err);
			}
		}),
		CONCURRENCY,
	);

	await save(env, result, suspects, timeout);
	return result;
}

/** History rows whose server id ends in one of `hexes`, keyed by that hex. */
async function lookup(env: Env, hexes: string[], timeout: number): Promise<Map<string, HistoryRow>> {
	const out = new Map<string, HistoryRow>();
	if (!hexes.length) return out;
	const stmts: D1PreparedStatement[] = [];
	for (let i = 0; i < hexes.length; i += D1_BATCH) {
		const chunk = hexes.slice(i, i + D1_BATCH);
		stmts.push(
			env.DB.prepare(`SELECT server_id, version, ended_at FROM sessions WHERE substr(server_id, -32) IN (${chunk.map(() => "?").join(", ")})`).bind(
				...chunk,
			),
		);
	}
	for (const r of await bounded(timeout, () => env.DB.batch<HistoryRow>(stmts))) {
		for (const row of r.results) out.set(row.server_id.slice(-32), row);
	}
	return out;
}

async function save(env: Env, r: SweepResult, suspects: Record<string, number>, timeout: number): Promise<void> {
	try {
		await bounded(timeout, () =>
			env.DB.prepare(
				`INSERT INTO sweep (id, at, listed, stopped, failed, error, suspects) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
				 ON CONFLICT (id) DO UPDATE SET at = excluded.at, listed = excluded.listed, stopped = excluded.stopped,
				   failed = excluded.failed, error = excluded.error, suspects = excluded.suspects`,
			)
				.bind(r.at, r.listed, r.stopped, r.failed, r.error, JSON.stringify(suspects))
				.run(),
		);
	} catch (err) {
		console.error("sweep state write failed", diag(err));
	}
}

/** The last sweep's result, for /admin; null before the first sweep or if D1 does not answer. */
export async function lastSweep(env: Env): Promise<SweepResult | null> {
	try {
		return await env.DB.prepare("SELECT at, listed, stopped, failed, error FROM sweep WHERE id = 1").first<SweepResult>();
	} catch (err) {
		console.error("sweep state read failed", diag(err));
		return null;
	}
}
